//! System-audio capture ([recording.md](../../../../docs/recording.md)):
//! a Core Audio process tap (macOS 14.4+).
//!
//! A `CATapDescription` asks for a mono global mixdown of every process
//! except our own (we must not re-record our own notification sounds),
//! marked private and unmuted (a muted tap would silence the user's
//! speakers). `AudioHardwareCreateProcessTap` creates the tap; the OS
//! shows its consent prompt ("System Audio Recording Only") on first use;
//! there is no public API to query that TCC state, so this whole module
//! is the probe: any failure returns `None` and the mixer records
//! mic-only. The tap is wrapped in a private aggregate device whose
//! IOProc delivers the tapped audio; frames run through the portable
//! pipeline into the mixer's sink at 16 kHz mono.
//!
//! Teardown order matters (stop → IOProc → aggregate → tap); a leaked
//! aggregate would persist in the HAL until process exit.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_audio::{
    kAudioHardwarePropertyTranslatePIDToProcessObject, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioTapPropertyFormat,
    AudioDeviceCreateIOProcIDWithBlock, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID,
    AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice,
    AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice,
    AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectPropertySelector, CATapDescription,
    CATapMuteBehavior,
};
use objc2_core_audio_types::{AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

use crate::audio::pipeline::Pipeline;

/// A running system-audio capture feeding 16 kHz mono blocks into its sink;
/// capture stops (and the tap tears down) when this is dropped.
///
/// Not `Send` (it owns the IO block): built and owned by the recorder's
/// dedicated system-audio thread, like the Windows loopback stream,
/// which also absorbs the consent-blocked first creation.
pub struct SystemAudioCapture {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    io_proc: AudioDeviceIOProcID,
    /// Keeps the block alive for the IOProc's lifetime (the HAL also
    /// copies it; belt and suspenders).
    _block: block2::RcBlock<
        dyn Fn(
            NonNull<AudioTimeStamp>,
            NonNull<AudioBufferList>,
            NonNull<AudioTimeStamp>,
            NonNull<AudioBufferList>,
            NonNull<AudioTimeStamp>,
        ),
    >,
}

impl SystemAudioCapture {
    /// Hold the system-audio capture until `stop_rx` closes: the platform
    /// layer's blocking entry point (the Windows side supervises reopens in
    /// here; the macOS tap already follows the default output, so this is
    /// start-once, announce, park, drop).
    pub fn run(
        sink_factory: crate::platform::types::SystemAudioSinkFactory,
        paused: Arc<AtomicBool>,
        preferred_device: Option<&str>,
        // The source picker's selection, ignored here: the tap is one
        // global mixdown, so there is no per-app stream to narrow to.
        // Per-process `CATapDescription`s are the macOS analogue and are
        // backlogged ([backlog.md]).
        _wanted: Box<dyn Fn() -> crate::platform::types::SystemAudioWanted + Send>,
        stop_rx: std::sync::mpsc::Receiver<crate::platform::types::CaptureCommand>,
        on_source: Box<dyn Fn(crate::platform::types::SystemAudioSource) + Send>,
    ) {
        let capture = Self::start(sink_factory(), paused, preferred_device);
        if capture.is_none() {
            tracing::warn!("system-audio capture unavailable — recording mic only");
            return;
        }
        // The global tap already captures everything the machine plays,
        // with no per-device notion to report.
        on_source(crate::platform::types::SystemAudioSource::Everything { devices: 0 });
        // Park until the recorder stops (the channel closing). A
        // `Reconfigure` is a picker change this platform cannot act on, so
        // it wakes us and we go straight back to parking.
        while stop_rx.recv().is_ok() {}
    }

    /// Start capturing everything the machine plays (except us). `None` on
    /// any failure (consent refused, no output hardware, an unexpected
    /// format), and the mixer degrades to mic-only. `preferred_device` is
    /// ignored: the global tap follows the default output like the Windows
    /// Everything mode; per-device taps are a later refinement.
    pub fn start(
        sink: Box<dyn Fn(&[f32]) + Send>,
        paused: Arc<AtomicBool>,
        _preferred_device: Option<&str>,
    ) -> Option<Self> {
        let own_process = translate_pid_to_process_object(std::process::id() as i32)?;

        // Mono mixdown of the world minus us, private, unmuted.
        let desc = unsafe {
            let exclude = NSArray::from_retained_slice(&[NSNumber::new_u32(own_process)]);
            CATapDescription::initMonoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &exclude,
            )
        };
        unsafe {
            desc.setName(&NSString::from_str("embral system audio"));
            desc.setPrivate(true);
            desc.setMuteBehavior(CATapMuteBehavior::Unmuted);
        }

        let mut tap: AudioObjectID = 0;
        let status = unsafe { AudioHardwareCreateProcessTap(Some(&desc), &mut tap) };
        if status != 0 || tap == 0 {
            tracing::warn!(status, "process tap refused (consent or hardware) — mic only");
            return None;
        }
        // From here on, failures must unwind what exists so nothing leaks
        // in the HAL.
        let torn_down = |tap: AudioObjectID| {
            unsafe { AudioHardwareDestroyProcessTap(tap) };
            None
        };

        let Some(format) = read_tap_format(tap) else {
            tracing::warn!("tap created but its format is unreadable — mic only");
            return torn_down(tap);
        };
        let is_float = format.mFormatFlags & objc2_core_audio_types::kAudioFormatFlagIsFloat != 0;
        let channels = format.mChannelsPerFrame.max(1) as usize;
        if !is_float || format.mBitsPerChannel != 32 {
            tracing::warn!(
                bits = format.mBitsPerChannel,
                float = is_float,
                "tap format isn't float32 — mic only"
            );
            return torn_down(tap);
        }
        tracing::info!(
            rate = format.mSampleRate,
            channels,
            "process tap created (mono mixdown requested)"
        );

        let Ok(pipeline) = Pipeline::new(channels, format.mSampleRate as u32) else {
            return torn_down(tap);
        };

        // The private aggregate device hosting the tap. Keys are the HAL's
        // own C-string constants; NSDictionary bridges toll-free to the
        // CFDictionary the create call wants.
        let aggregate_uid = uuid::Uuid::new_v4().to_string();
        let tap_uid = unsafe { desc.UUID().UUIDString() };
        let description: Retained<NSDictionary<NSString, objc2::runtime::AnyObject>> = {
            let sub_tap = NSDictionary::from_retained_objects(
                &[&*key(objc2_core_audio::kAudioSubTapUIDKey)],
                &[any(tap_uid)],
            );
            let taps = NSArray::from_retained_slice(&[sub_tap]);
            NSDictionary::from_retained_objects(
                &[
                    &*key(objc2_core_audio::kAudioAggregateDeviceUIDKey),
                    &*key(objc2_core_audio::kAudioAggregateDeviceNameKey),
                    &*key(objc2_core_audio::kAudioAggregateDeviceIsPrivateKey),
                    &*key(objc2_core_audio::kAudioAggregateDeviceTapListKey),
                ],
                &[
                    any(NSString::from_str(&aggregate_uid)),
                    any(NSString::from_str("embral system audio")),
                    any(NSNumber::new_i32(1)),
                    any(taps),
                ],
            )
        };
        let mut aggregate: AudioObjectID = 0;
        let status = unsafe {
            AudioHardwareCreateAggregateDevice(
                &*(Retained::as_ptr(&description) as *const objc2_core_foundation::CFDictionary),
                NonNull::from(&mut aggregate),
            )
        };
        if status != 0 || aggregate == 0 {
            tracing::warn!(status, "aggregate device for the tap failed — mic only");
            return torn_down(tap);
        }

        // The IO block: tapped frames → pipeline → the mixer's sink.
        // Runs on the HAL's IO thread; same non-blocking discipline as the
        // cpal callbacks.
        let pipeline = std::sync::Mutex::new(pipeline);
        let block = block2::RcBlock::new(
            move |_now: NonNull<AudioTimeStamp>,
                  input: NonNull<AudioBufferList>,
                  _in_time: NonNull<AudioTimeStamp>,
                  _output: NonNull<AudioBufferList>,
                  _out_time: NonNull<AudioTimeStamp>| {
                if paused.load(Ordering::SeqCst) {
                    return;
                }
                let list = unsafe { input.as_ref() };
                let buffers = unsafe {
                    std::slice::from_raw_parts(
                        list.mBuffers.as_ptr(),
                        list.mNumberBuffers as usize,
                    )
                };
                let Ok(mut pipeline) = pipeline.lock() else { return };
                for buffer in buffers {
                    if buffer.mData.is_null() {
                        continue;
                    }
                    let samples = unsafe {
                        std::slice::from_raw_parts(
                            buffer.mData as *const f32,
                            buffer.mDataByteSize as usize / std::mem::size_of::<f32>(),
                        )
                    };
                    pipeline.push(samples, &mut |resampled| sink(resampled));
                }
            },
        );

        let mut io_proc: AudioDeviceIOProcID = None;
        let status = unsafe {
            AudioDeviceCreateIOProcIDWithBlock(
                NonNull::from(&mut io_proc),
                aggregate,
                None,
                &*block as *const _ as *mut _,
            )
        };
        if status != 0 || io_proc.is_none() {
            tracing::warn!(status, "tap IOProc failed — mic only");
            unsafe { AudioHardwareDestroyAggregateDevice(aggregate) };
            return torn_down(tap);
        }
        let status = unsafe { AudioDeviceStart(aggregate, io_proc) };
        if status != 0 {
            tracing::warn!(status, "tap device start failed — mic only");
            unsafe {
                AudioDeviceDestroyIOProcID(aggregate, io_proc);
                AudioHardwareDestroyAggregateDevice(aggregate);
            }
            return torn_down(tap);
        }
        tracing::info!("System-audio tap started");

        Some(Self {
            tap,
            aggregate,
            io_proc,
            _block: block,
        })
    }
}

impl Drop for SystemAudioCapture {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.aggregate, self.io_proc);
            AudioDeviceDestroyIOProcID(self.aggregate, self.io_proc);
            AudioHardwareDestroyAggregateDevice(self.aggregate);
            AudioHardwareDestroyProcessTap(self.tap);
        }
    }
}

/// An NSString key from one of the HAL's C-string dictionary constants.
fn key(c: &std::ffi::CStr) -> Retained<NSString> {
    NSString::from_str(c.to_str().expect("HAL key is ascii"))
}

/// Erase a dictionary value to `AnyObject` (the values are heterogeneous).
fn any<T: objc2::Message + 'static>(obj: Retained<T>) -> Retained<objc2::runtime::AnyObject> {
    unsafe { Retained::cast_unchecked(obj) }
}

/// Our pid's HAL process object (the tap's exclusion list wants object
/// ids, not pids).
fn translate_pid_to_process_object(pid: i32) -> Option<AudioObjectID> {
    let mut addr = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut object: AudioObjectID = 0;
    let mut size = std::mem::size_of::<AudioObjectID>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&mut addr),
            std::mem::size_of::<i32>() as u32,
            &pid as *const i32 as *const c_void,
            NonNull::from(&mut size),
            NonNull::new(&mut object as *mut AudioObjectID as *mut c_void)?,
        )
    };
    (status == 0 && object != 0).then_some(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live probe of the real tap; run manually while something plays
    /// audio: `cargo test -p embral --lib tap_captures -- --ignored --nocapture`.
    /// First run fires the system-audio consent prompt; on a machine with
    /// no output hardware this instead proves the degrade path (a clean
    /// `None`, no leaks, no crash).
    #[test]
    #[ignore = "manual probe; needs consent + something playing audio"]
    fn tap_captures_system_audio() {
        let frames = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = frames.clone();
        let capture = SystemAudioCapture::start(
            Box::new(move |block| {
                counter.fetch_add(block.len(), Ordering::Relaxed);
            }),
            Arc::new(AtomicBool::new(false)),
            None,
        );
        match capture {
            None => eprintln!("tap unavailable — degrade path exercised"),
            Some(capture) => {
                std::thread::sleep(std::time::Duration::from_secs(3));
                drop(capture);
                eprintln!(
                    "tap delivered {} resampled samples in ~3 s",
                    frames.load(Ordering::Relaxed)
                );
            }
        }
    }
}

/// The tap's delivered stream format.
fn read_tap_format(tap: AudioObjectID) -> Option<AudioStreamBasicDescription> {
    let mut addr = AudioObjectPropertyAddress {
        mSelector: kAudioTapPropertyFormat as AudioObjectPropertySelector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    // POD struct; zero is the conventional "fill me in" starting value.
    let mut format: AudioStreamBasicDescription = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            tap,
            NonNull::from(&mut addr),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new(&mut format as *mut _ as *mut c_void)?,
        )
    };
    (status == 0).then_some(format)
}

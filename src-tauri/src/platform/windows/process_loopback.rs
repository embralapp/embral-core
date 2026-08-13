//! Per-process system-audio capture ([recording.md]): the capture path that
//! fixes "embral captured the monitor while Zoom played out of the laptop".
//!
//! `ActivateAudioInterfaceAsync` with `AUDIOCLIENT_ACTIVATION_TYPE_
//! PROCESS_LOOPBACK` captures one process tree's render streams wherever
//! they play: no endpoint is involved, so device changes cannot desync
//! it, and audio from every other app (music, notifications) stays out of
//! the recording. Windows build ≥ 20348 only; older builds fall back to
//! the device capture in `loopback.rs`.
//!
//! The mic-session pid detection reports is often a renderer child while
//! the audio plays from a sibling (every browser), so the target is the
//! topmost ancestor sharing the same executable name, captured with
//! `INCLUDE_TARGET_PROCESS_TREE`.
//!
//! The WASAPI glue here cannot be unit-tested (it needs a real process
//! rendering audio); the pure parts (the tree climb and the build gate)
//! are tested below, and the whole path degrades to the device capture on
//! any failure.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

use windows::core::{implement, IUnknownImpl, Interface, Ref, Result as WinResult};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0};
use windows::Win32::System::Com::BLOB;
use windows::Win32::System::Variant::VT_BLOB;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

use crate::audio::pipeline::Pipeline;

/// The build that introduced process loopback (Windows 10 21H2 server /
/// Windows 11 client). Older builds get the device capture.
const MIN_BUILD: u32 = 20348;

/// The format we ask the capture engine for. Shared mode converts and
/// mixes into whatever we name, so one canonical shape avoids a
/// format-negotiation dance; the portable pipeline resamples to 16 kHz.
const CAPTURE_RATE: u32 = 48_000;
const CAPTURE_CHANNELS: u16 = 2;

/// Whether this Windows build supports process loopback at all.
pub(crate) fn supported(os_build: &str) -> bool {
    build_number(os_build).is_some_and(|b| b >= MIN_BUILD)
}

/// One app's capture, running on its own thread until dropped.
///
/// The pump owns `!Send` COM handles, so it cannot live on the caller's
/// thread beside the others; each app gets a thread that opens, pumps,
/// and tears down. Dropping the handle stops it; `alive()` reports a pump
/// that ended on its own (the app quit, or the capture failed).
pub(crate) struct AppCapture {
    pub pid: u32,
    pub name: String,
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AppCapture {
    /// Start capturing this app's tree. `None` when the build is too old,
    /// the app is gone, or activation fails; the caller skips it and
    /// keeps every other source running.
    pub(crate) fn start(
        pid: u32,
        sink: Box<dyn Fn(&[f32]) + Send>,
        paused: Arc<AtomicBool>,
    ) -> Option<Self> {
        if !supported(&super::os_build::os_build()) {
            tracing::info!("per-app capture needs a newer Windows build");
            return None;
        }
        let name = super::mic_users::process_name(pid)?;
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let running = Arc::new(AtomicBool::new(true));
        let ready = Arc::new(AtomicBool::new(false));

        let thread_running = running.clone();
        let thread_ready = ready.clone();
        let thread = std::thread::Builder::new()
            .name(format!("app-audio-{pid}"))
            .spawn(move || {
                // A panic here must not poison the process: the capture
                // just reports dead and the supervisor recaptures or falls
                // back to capturing everything. (Memory corruption inside
                // the OS call is not catchable; see recording.md.)
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    pump(pid, sink, paused, &stop_rx, &thread_ready)
                }));
                if outcome.is_err() {
                    tracing::error!(pid, "app capture panicked — dropping this source");
                }
                thread_running.store(false, Ordering::SeqCst);
            })
            .ok()?;

        // Activation is quick when it works; give it a moment so a failed
        // start is reported now rather than as a source that never speaks.
        for _ in 0..40 {
            if ready.load(Ordering::SeqCst) {
                return Some(Self {
                    pid,
                    name,
                    stop_tx: Some(stop_tx),
                    running,
                    thread: Some(thread),
                });
            }
            if !running.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        tracing::warn!(pid, "app capture did not start");
        drop(stop_tx);
        let _ = thread.join();
        None
    }

    pub(crate) fn alive(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

impl Drop for AppCapture {
    fn drop(&mut self) {
        self.stop_tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The build number out of the platform's `os_build` string (e.g.
/// "26200" or "10.0.26200"); `None` when it is not a number at all.
fn build_number(os_build: &str) -> Option<u32> {
    os_build
        .rsplit('.')
        .next()
        .and_then(|last| last.trim().parse::<u32>().ok())
}

/// One row of the process table: (pid, parent pid, executable name).
type ProcRow = (u32, u32, String);

/// The topmost ancestor of `start` that shares its executable name: a
/// browser's mic-holding renderer resolves to the browser itself, so
/// include-tree capture then covers every sibling that plays audio.
/// Unknown pids and parent cycles resolve to `start` unchanged.
pub(crate) fn climb(table: &[ProcRow], start: u32) -> u32 {
    let exe_of = |pid: u32| {
        table
            .iter()
            .find(|(p, _, _)| *p == pid)
            .map(|(_, _, exe)| exe.to_lowercase())
    };
    let parent_of = |pid: u32| table.iter().find(|(p, _, _)| *p == pid).map(|(_, pp, _)| *pp);

    let Some(name) = exe_of(start) else {
        return start;
    };
    let mut best = start;
    let mut cursor = start;
    // The table is small (a few hundred rows) and every hop must be a new
    // pid, so the visited set is the cycle guard.
    let mut seen = vec![start];
    while let Some(parent) = parent_of(cursor) {
        if parent == 0 || seen.contains(&parent) {
            break;
        }
        seen.push(parent);
        if exe_of(parent).as_deref() == Some(name.as_str()) {
            best = parent;
        }
        cursor = parent;
    }
    best
}

/// Snapshot the process table (pid, parent, exe name).
fn process_table() -> WinResult<Vec<ProcRow>> {
    let mut rows = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                rows.push((
                    entry.th32ProcessID,
                    entry.th32ParentProcessID,
                    String::from_utf16_lossy(&entry.szExeFile[..len]),
                ));
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    Ok(rows)
}

/// Waits for `ActivateAudioInterfaceAsync` to finish. The activation call
/// returns immediately and the completion lands on an MTA worker, so the
/// handler just signals an event our thread waits on.
///
/// The handler owns the event: the runtime holds its own reference for as
/// long as the activation is in flight, so closing the handle anywhere else
/// would leave `ActivateCompleted` signalling a recycled handle when an
/// activation we gave up waiting for finally lands.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationHandler {
    done: HANDLE,
}

impl Drop for ActivationHandler {
    fn drop(&mut self) {
        unsafe {
            if !self.done.is_invalid() {
                let _ = CloseHandle(self.done);
            }
        }
    }
}

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> WinResult<()> {
        unsafe {
            let _ = SetEvent(self.get_impl().done);
        }
        Ok(())
    }
}

/// A live per-process capture. Dropping it stops and releases everything.
struct ProcessCapture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: HANDLE,
}

impl Drop for ProcessCapture {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            if !self.event.is_invalid() {
                let _ = CloseHandle(self.event);
            }
        }
        // `capture` and `client` release with the struct.
        let _ = &self.capture;
    }
}

/// Activate and start a process-loopback capture for `target_pid`'s tree.
///
/// COM must already be initialized (MTA) on this thread: the activation
/// is asynchronous and its completion calls back into a COM object we
/// host, which is not safe on an uninitialized thread.
fn open(target_pid: u32) -> WinResult<ProcessCapture> {
    unsafe {
        let done = CreateEventW(None, true, false, None)?;
        let handler: IActivateAudioInterfaceCompletionHandler =
            ActivationHandler { done }.into();

        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: target_pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        // The params ride in a PROPVARIANT as a raw blob (the documented
        // shape for this API). Built by hand rather than through a
        // constructor: no safe wrapper covers VT_BLOB. The blob borrows
        // `params` on our stack, which outlives the call below.
        //
        // `ManuallyDrop` is required here, not tidiness: windows-rs gives
        // PROPVARIANT a `Drop` that calls `PropVariantClear`, and for
        // VT_BLOB that hands `pBlobData` to `CoTaskMemFree`. That is a
        // stack address, which corrupts the heap and kills the process at
        // some later allocation (0xc0000374, twice in the field). This variant
        // owns nothing, so its destructor must never run.
        let mut inner = PROPVARIANT_0_0::default();
        inner.vt = VT_BLOB;
        inner.Anonymous.blob = BLOB {
            cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            pBlobData: &mut params as *mut _ as *mut u8,
        };
        let variant = std::mem::ManuallyDrop::new(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(inner),
            },
        });

        let operation: IActivateAudioInterfaceAsyncOperation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&*variant as *const PROPVARIANT),
            &handler,
        )?;

        // The completion handler signals; 3 s is generous for an
        // activation that normally completes in milliseconds. Giving up
        // does not close the event: the handler owns it, and the runtime
        // may still hold a reference to a late activation.
        let signaled = WaitForSingleObject(done, 3_000) == WAIT_OBJECT_0;
        let result = if signaled {
            let mut activate_result = windows::core::HRESULT(0);
            let mut unknown = None;
            operation
                .GetActivateResult(&mut activate_result, &mut unknown)
                .and_then(|()| activate_result.ok())
                .map(|()| unknown)
        } else {
            Err(windows::core::Error::empty())
        };
        drop(operation);
        drop(handler);
        let unknown = result?;
        let client: IAudioClient = unknown
            .ok_or_else(windows::core::Error::empty)?
            .cast()?;

        // Shared-mode float PCM; the engine converts the app's own format.
        // WAVE_FORMAT_IEEE_FLOAT (3) has no binding in this crate version.
        let mut format = WAVEFORMATEX {
            wFormatTag: 3,
            nChannels: CAPTURE_CHANNELS,
            nSamplesPerSec: CAPTURE_RATE,
            wBitsPerSample: 32,
            ..Default::default()
        };
        format.nBlockAlign = format.nChannels * format.wBitsPerSample / 8;
        format.nAvgBytesPerSec = format.nSamplesPerSec * format.nBlockAlign as u32;

        // 200 ms of buffer: comfortably more than the drain cadence.
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            2_000_000,
            0,
            &format,
            None,
        )?;

        let event = CreateEventW(None, false, false, None)?;
        client.SetEventHandle(event)?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;

        Ok(ProcessCapture {
            client,
            capture,
            event,
        })
    }
}

/// Open the capture and pump it until the stop signal or a failure.
/// `ready` flips once audio can flow, so the caller knows the difference
/// between "starting" and "never going to start".
fn pump(
    target_pid: u32,
    sink: Box<dyn Fn(&[f32]) + Send>,
    paused: Arc<AtomicBool>,
    stop_rx: &Receiver<()>,
    ready: &Arc<AtomicBool>,
) {
    // COM on this thread, MTA: the activation below is asynchronous and
    // calls back into a COM object we host. Without this the callback
    // arrives on a thread the runtime knows nothing about; it corrupted the
    // heap in the field before this line existed. RPC_E_CHANGED_MODE
    // (already initialized differently) is fine for our usage.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
    }

    let table = match process_table() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("process table unavailable ({e})");
            return;
        }
    };
    let tree_root = climb(&table, target_pid);
    let capture = match open(tree_root) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                pid = target_pid,
                root = tree_root,
                "per-process capture unavailable ({e})"
            );
            return;
        }
    };

    let mut pipeline = match Pipeline::new(CAPTURE_CHANNELS as usize, CAPTURE_RATE) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("resampler unavailable ({e})");
            return;
        }
    };
    tracing::info!(
        pid = target_pid,
        root = tree_root,
        "per-app system audio started"
    );
    ready.store(true, Ordering::SeqCst);

    loop {
        match stop_rx.recv_timeout(Duration::from_millis(0)) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return, // stop signal, or the recorder is gone
        }
        unsafe {
            // Wake on data or every 200 ms, so the stop check stays live.
            let _ = WaitForSingleObject(capture.event, 200);
            loop {
                let frames = match capture.capture.GetNextPacketSize() {
                    Ok(f) => f,
                    Err(_) => return,
                };
                if frames == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut count = 0u32;
                let mut flags = 0u32;
                if capture
                    .capture
                    .GetBuffer(&mut data, &mut count, &mut flags, None, None)
                    .is_err()
                {
                    return;
                }
                if !paused.load(Ordering::SeqCst) && !data.is_null() && count > 0 {
                    let samples = std::slice::from_raw_parts(
                        data as *const f32,
                        count as usize * CAPTURE_CHANNELS as usize,
                    );
                    pipeline.push(samples, &mut |resampled| sink(resampled));
                }
                let _ = capture.capture.ReleaseBuffer(count);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, parent: u32, exe: &str) -> ProcRow {
        (pid, parent, exe.to_string())
    }

    #[test]
    fn a_browser_renderer_climbs_to_the_browser() {
        // The mic session lives in a renderer; the audio plays from a
        // sibling, so capture must target the top chrome.exe.
        let table = vec![
            row(4, 0, "System"),
            row(100, 4, "explorer.exe"),
            row(200, 100, "chrome.exe"),
            row(300, 200, "chrome.exe"),
            row(400, 300, "chrome.exe"),
        ];
        assert_eq!(climb(&table, 400), 200);
        assert_eq!(climb(&table, 200), 200);
    }

    #[test]
    fn a_single_process_app_stays_itself() {
        let table = vec![row(100, 4, "explorer.exe"), row(500, 100, "Zoom.exe")];
        assert_eq!(climb(&table, 500), 500);
    }

    #[test]
    fn unknown_pids_and_cycles_resolve_to_the_start() {
        let table = vec![row(100, 4, "chrome.exe")];
        assert_eq!(climb(&table, 999), 999);
        // A parent cycle must not hang the climb.
        let cyclic = vec![row(10, 20, "a.exe"), row(20, 10, "a.exe")];
        assert_eq!(climb(&cyclic, 10), 20);
    }

    #[test]
    fn the_build_gate_reads_platform_strings() {
        assert!(supported("26200"));
        assert!(supported("10.0.26200"));
        assert!(supported("20348"));
        assert!(!supported("19045")); // Windows 10 22H2
        assert!(!supported("unknown"));
    }
}

//! Live spectrum metering over the recorder's pre-mix audio blocks.
//!
//! Runs in the mic callback, where the mic and system-loopback blocks still
//! exist separately, and feeds the recording view's per-band level meter.
//! Timing is derived from consumed sample counts, exactly like the WAV and
//! transcript timelines.

const SAMPLE_RATE: u64 = super::SAMPLE_RATE_HZ as u64;

/// Frequency bands for the live spectrum meter, log-spaced across the
/// vocal fundamental range. The frontend renders one stationary bar per
/// band.
pub const LEVEL_BANDS: usize = 24;
const BAND_LOW_HZ: f32 = 85.0;
const BAND_HIGH_HZ: f32 = 500.0;

/// The band center frequencies (log-spaced low→high).
pub fn band_frequencies() -> [f32; LEVEL_BANDS] {
    let ratio = BAND_HIGH_HZ / BAND_LOW_HZ;
    std::array::from_fn(|i| {
        BAND_LOW_HZ * ratio.powf(i as f32 / (LEVEL_BANDS - 1) as f32)
    })
}

/// Normalized single-frequency magnitude (Goertzel) of a sample slice,
/// a tiny filterbank beats pulling in an FFT for two dozen bands.
fn goertzel_magnitude(samples: &[f32], freq: f32) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let w = 2.0 * std::f32::consts::PI * freq / SAMPLE_RATE as f32;
    let coeff = 2.0 * w.cos();
    let (mut s_prev, mut s_prev2) = (0.0f32, 0.0f32);
    for &x in samples {
        let s = x + coeff * s_prev - s_prev2;
        s_prev2 = s_prev;
        s_prev = s;
    }
    let power =
        (s_prev2 * s_prev2 + s_prev * s_prev - coeff * s_prev * s_prev2).max(0.0);
    2.0 * power.sqrt() / samples.len() as f32
}

/// Live spectrum tap for the recording view's meter: folds the same pre-mix
/// blocks into ~100 ms slices and hands each slice's per-band mic/system
/// magnitudes to a callback (the Tauri event emitter). Sample-counted like
/// everything else, so a paused stream (no blocks) emits nothing.
pub struct LevelTap {
    cb: Box<dyn Fn(&[f32], &[f32]) + Send>,
    mic_buf: Vec<f32>,
    loop_buf: Vec<f32>,
}

/// ~100 ms at 16 kHz → 10 slices/second.
const LEVEL_SLICE_SAMPLES: usize = SAMPLE_RATE as usize / 10;

impl LevelTap {
    pub fn new(cb: Box<dyn Fn(&[f32], &[f32]) + Send>) -> Self {
        Self {
            cb,
            mic_buf: Vec::new(),
            loop_buf: Vec::new(),
        }
    }

    /// Consume one mic block and the loopback samples mixed against it
    /// (`lb` may be shorter; the missing tail is silence).
    pub fn push_block(&mut self, mic: &[f32], lb: &[f32]) {
        self.mic_buf.extend_from_slice(mic);
        self.loop_buf.extend_from_slice(lb);
        // Keep the two channels sample-aligned.
        self.loop_buf.resize(self.mic_buf.len(), 0.0);

        while self.mic_buf.len() >= LEVEL_SLICE_SAMPLES {
            let mic_slice: Vec<f32> = self.mic_buf.drain(..LEVEL_SLICE_SAMPLES).collect();
            let loop_slice: Vec<f32> = self.loop_buf.drain(..LEVEL_SLICE_SAMPLES).collect();
            let freqs = band_frequencies();
            let mic_bands: Vec<f32> =
                freqs.iter().map(|&f| goertzel_magnitude(&mic_slice, f)).collect();
            let loop_bands: Vec<f32> =
                freqs.iter().map(|&f| goertzel_magnitude(&loop_slice, f)).collect();
            (self.cb)(&mic_bands, &loop_bands);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn level_tap_emits_band_spectra_per_slice() {
        let seen: Arc<Mutex<Vec<(Vec<f32>, Vec<f32>)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let mut tap = LevelTap::new(Box::new(move |m, l| {
            sink.lock().unwrap().push((m.to_vec(), l.to_vec()))
        }));

        // One second: a 440 Hz tone on the mic, silence on the loopback
        // (shorter blocks, when input ran dry, pad as silence).
        let mic: Vec<f32> = (0..1024)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin() * 0.5)
            .collect();
        for _ in 0..16 {
            tap.push_block(&mic, &[]);
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 10, "10 Hz for one second of samples");
        let (mic_bands, loop_bands) = &seen[0];
        assert_eq!(mic_bands.len(), LEVEL_BANDS);
        assert_eq!(loop_bands.len(), LEVEL_BANDS);
        // The loudest mic band sits nearest 440 Hz; the loopback is silent.
        let freqs = band_frequencies();
        let loudest = mic_bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap();
        assert!(
            (freqs[loudest] - 440.0).abs() < 150.0,
            "loudest band at {} Hz",
            freqs[loudest]
        );
        assert!(loop_bands.iter().all(|&v| v < 1e-6));
    }

    #[test]
    fn level_tap_holds_partial_slices() {
        let count = Arc::new(Mutex::new(0usize));
        let sink = count.clone();
        let mut tap = LevelTap::new(Box::new(move |_, _| *sink.lock().unwrap() += 1));
        tap.push_block(&vec![0.3f32; 1000], &[]);
        assert_eq!(*count.lock().unwrap(), 0, "under one slice → nothing");
        tap.push_block(&vec![0.3f32; 1000], &[]);
        assert_eq!(*count.lock().unwrap(), 1);
    }
}

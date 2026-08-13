//! The portable capture pipeline: interleaved device frames in, 16 kHz
//! mono blocks out ([recording.md](../../../docs/recording.md)).
//!
//! Every capture source (the cpal mic stream, the WASAPI loopback trick,
//! the macOS process tap) feeds this same pipeline, so the mixer only
//! ever sees canonical 16 kHz mono and the platform layer never resamples.
//! Pure over its inputs: no devices, no OS, unit-tested.

use anyhow::{anyhow, Result};
use rubato::{FftFixedInOut, Resampler};

pub use super::SAMPLE_RATE_HZ as TARGET_SAMPLE_RATE;
/// The resampler's chunking parameter. Emitted block sizes scale with the
/// rate ratio (48 kHz in → ~342-sample 16 kHz blocks), fixed per stream,
/// not equal to this constant.
pub const RESAMPLE_CHUNK: usize = 1024;

/// Downmix → accumulate → resample. One instance per capture stream; not
/// shared (each device callback owns its pipeline and calls it serially).
pub struct Pipeline {
    channels: usize,
    resampler: FftFixedInOut<f32>,
    /// Mono samples at the device rate awaiting a full resampler input.
    acc: Vec<f32>,
    /// Device-rate frames the resampler consumes per chunk.
    required: usize,
}

impl Pipeline {
    pub fn new(channels: usize, device_rate: u32) -> Result<Self> {
        if channels == 0 {
            return Err(anyhow!("zero-channel stream"));
        }
        let resampler = FftFixedInOut::<f32>::new(
            device_rate as usize,
            TARGET_SAMPLE_RATE as usize,
            RESAMPLE_CHUNK,
            1,
        )
        .map_err(|e| anyhow!("Failed to create resampler: {}", e))?;
        let required = resampler.input_frames_next();
        Ok(Self {
            channels,
            resampler,
            acc: Vec::new(),
            required,
        })
    }

    /// Device-rate frames consumed per emitted chunk (diagnostics).
    pub fn required_input_frames(&self) -> usize {
        self.required
    }

    /// Feed one interleaved f32 block; `emit` receives each completed
    /// 16 kHz mono chunk (zero or more per call; the remainder waits).
    pub fn push(&mut self, interleaved: &[f32], emit: &mut dyn FnMut(&[f32])) {
        self.acc.extend(interleaved.chunks(self.channels).map(|frame| {
            let sum: f32 = frame.iter().sum();
            sum / self.channels as f32
        }));

        while self.acc.len() >= self.required {
            let chunk: Vec<f32> = self.acc.drain(..self.required).collect();
            match self.resampler.process(&[chunk], None) {
                Ok(output) => emit(&output[0]),
                Err(e) => tracing::error!("Resampler error: {}", e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Interleave a stereo pair pattern for `frames` frames.
    fn stereo(frames: usize, left: f32, right: f32) -> Vec<f32> {
        (0..frames).flat_map(|_| [left, right]).collect()
    }

    #[test]
    fn downmixes_and_emits_fixed_chunks() {
        let mut p = Pipeline::new(2, 48000).expect("pipeline");
        let required = p.required_input_frames();

        let mut emitted: Vec<usize> = Vec::new();
        let mut last_mean = 0.0f32;
        // Three full inputs plus a remainder that must stay buffered.
        let input = stereo(required * 3 + 7, 0.8, 0.0);
        p.push(&input, &mut |chunk| {
            emitted.push(chunk.len());
            last_mean = chunk.iter().sum::<f32>() / chunk.len() as f32;
        });

        // One fixed-size output block per full input; 48k→16k scales the
        // block to a third of the input frames.
        assert_eq!(emitted.len(), 3);
        let block = emitted[0];
        assert!(emitted.iter().all(|&n| n == block), "blocks are fixed-size");
        assert!(
            (block as f64 - required as f64 / 3.0).abs() < 2.0,
            "16k block {block} should be ~1/3 of {required} input frames"
        );
        // L=0.8, R=0.0 downmixes to DC 0.4. The first block carries the
        // FFT filter's ramp-up, so steady state is judged on the last.
        assert!(
            (last_mean - 0.4).abs() < 0.02,
            "steady-state mean {last_mean} should be ~0.4"
        );
    }

    #[test]
    fn remainder_carries_into_the_next_push() {
        let mut p = Pipeline::new(1, 48000).expect("pipeline");
        let required = p.required_input_frames();

        let mut chunks = 0;
        p.push(&vec![0.1; required - 1], &mut |_| chunks += 1);
        assert_eq!(chunks, 0, "one frame short must not emit");
        p.push(&[0.1], &mut |_| chunks += 1);
        assert_eq!(chunks, 1, "the carried remainder completes a chunk");
    }

    #[test]
    fn unity_rate_passes_through_sized_chunks() {
        let mut p = Pipeline::new(1, TARGET_SAMPLE_RATE).expect("pipeline");
        let required = p.required_input_frames();
        let mut total = 0usize;
        p.push(&vec![0.5; required * 2], &mut |c| total += c.len());
        assert_eq!(total, RESAMPLE_CHUNK * 2);
    }
}

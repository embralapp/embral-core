/// The canonical rate everything downstream consumes (WAV, transcription,
/// meters): audio-seconds == samples / 16000 uniformly. Every capture path
/// resamples to it before fan-out, so this is the one place the number
/// lives. The typed aliases beside it exist because the meter counts in
/// `u64` and the transcription clocks divide in `f64`.
pub const SAMPLE_RATE_HZ: u32 = 16_000;

pub mod encoder;
pub mod meter;
pub mod pipeline;
pub mod recorder;

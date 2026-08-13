use anyhow::Result;
use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, MonoPcm, Quality};
use std::path::Path;

pub fn encode_wav_to_mp3(wav_path: &Path, mp3_path: &Path) -> Result<()> {
    let reader = hound::WavReader::open(wav_path)?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<std::result::Result<_, _>>()?,
        hound::SampleFormat::Int => reader
            .into_samples::<i32>()
            .map(|s| s.map(|v| v as f32 / i32::MAX as f32))
            .collect::<std::result::Result<_, _>>()?,
    };

    encode_samples_to_mp3(&samples, spec.sample_rate, mp3_path)
}

pub fn encode_samples_to_mp3(samples: &[f32], sample_rate: u32, mp3_path: &Path) -> Result<()> {
    let i16_samples: Vec<i16> = samples
        .iter()
        .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
        .collect();

    let mut encoder = Builder::new()
        .expect("Create LAME builder")
        .with_num_channels(1)
        .expect("set channels")
        .with_sample_rate(sample_rate)
        .expect("set sample rate")
        .with_brate(Bitrate::Kbps64)
        .expect("set bitrate")
        .with_quality(Quality::Best)
        .expect("set quality")
        .build()
        .expect("build encoder");

    let input = MonoPcm(i16_samples.as_slice());
    let mut mp3_buf = Vec::new();
    mp3_buf.reserve(mp3lame_encoder::max_required_buffer_size(i16_samples.len()));

    let encoded_size = encoder
        .encode(input, mp3_buf.spare_capacity_mut())
        .expect("encode");
    unsafe {
        mp3_buf.set_len(mp3_buf.len().wrapping_add(encoded_size));
    }

    let flush_size = encoder
        .flush::<FlushNoGap>(mp3_buf.spare_capacity_mut())
        .expect("flush");
    unsafe {
        mp3_buf.set_len(mp3_buf.len().wrapping_add(flush_size));
    }

    std::fs::write(mp3_path, &mp3_buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rate argument used to be discarded for a hardcoded 16 kHz, so a
    /// WAV at any other rate encoded at the wrong speed. The same samples at
    /// two rates are two different durations, so they cannot encode to the
    /// same number of frames; identical output means the argument is being
    /// ignored again.
    #[test]
    fn the_sample_rate_argument_reaches_the_encoder() {
        let dir = std::env::temp_dir().join(format!("embral-enc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let samples = vec![0.25f32; 16_000];

        let at_16k = dir.join("16k.mp3");
        let at_8k = dir.join("8k.mp3");
        encode_samples_to_mp3(&samples, 16_000, &at_16k).unwrap();
        encode_samples_to_mp3(&samples, 8_000, &at_8k).unwrap();

        let len_16k = std::fs::metadata(&at_16k).unwrap().len();
        let len_8k = std::fs::metadata(&at_8k).unwrap().len();
        assert_ne!(
            len_16k, len_8k,
            "both rates produced {len_16k} bytes, so the rate is being ignored"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

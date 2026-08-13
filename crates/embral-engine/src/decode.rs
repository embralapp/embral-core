//! Decode an audio file (wav/mp3/m4a/aac) to the engine's canonical format:
//! 16 kHz mono f32 PCM. Used by import; everything downstream of capture
//! (sessions, finalize, notes) then works exactly as it does for live audio.

use std::fs::File;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use rubato::{FftFixedInOut, Resampler};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const TARGET_RATE: usize = 16_000;
const RESAMPLE_CHUNK: usize = 1024;

/// Decode `path` to 16 kHz mono f32 samples.
pub fn decode_to_pcm16k(path: &Path) -> Result<Vec<f32>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("unrecognized or unsupported audio format")?;
    let mut format = probed.format;

    let track = format
        .default_track()
        .ok_or_else(|| anyhow!("no audio track in file"))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("unsupported audio codec")?;

    let source_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| anyhow!("unknown sample rate"))? as usize;

    // Decode everything to mono f32 at the source rate.
    let mut mono: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(symphonia::core::errors::Error::ResetRequired) => break,
            Err(e) => return Err(e).context("read audio packet"),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => append_mono(&decoded, &mut mono),
            // Recoverable decode errors (corrupt frame) are skipped.
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(e).context("decode audio packet"),
        }
    }

    if mono.is_empty() {
        bail!("file contained no decodable audio");
    }
    if source_rate == TARGET_RATE {
        return Ok(mono);
    }

    // Resample to 16 kHz using the same FFT resampler the recorder uses.
    let mut resampler = FftFixedInOut::<f32>::new(source_rate, TARGET_RATE, RESAMPLE_CHUNK, 1)
        .map_err(|e| anyhow!("create resampler: {e}"))?;
    let input_frames = resampler.input_frames_next();
    let mut out = Vec::with_capacity(mono.len() * TARGET_RATE / source_rate + TARGET_RATE);
    let mut pos = 0;
    while pos + input_frames <= mono.len() {
        let chunk = vec![mono[pos..pos + input_frames].to_vec()];
        let processed = resampler
            .process(&chunk, None)
            .map_err(|e| anyhow!("resample: {e}"))?;
        out.extend_from_slice(&processed[0]);
        pos += input_frames;
    }
    // Tail: pad the final partial chunk with silence.
    if pos < mono.len() {
        let mut tail = mono[pos..].to_vec();
        tail.resize(input_frames, 0.0);
        let processed = resampler
            .process(&[tail], None)
            .map_err(|e| anyhow!("resample tail: {e}"))?;
        out.extend_from_slice(&processed[0]);
    }
    Ok(out)
}

/// Downmix one decoded buffer to mono f32 and append it.
fn append_mono(decoded: &AudioBufferRef<'_>, mono: &mut Vec<f32>) {
    macro_rules! mix {
        ($buf:expr, $to_f32:expr) => {{
            let buf = $buf;
            let channels = buf.spec().channels.count();
            let frames = buf.frames();
            mono.reserve(frames);
            for frame in 0..frames {
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    sum += $to_f32(buf.chan(ch)[frame]);
                }
                mono.push(sum / channels as f32);
            }
        }};
    }
    match decoded {
        AudioBufferRef::F32(buf) => mix!(buf, |s: f32| s),
        AudioBufferRef::F64(buf) => mix!(buf, |s: f64| s as f32),
        AudioBufferRef::S32(buf) => mix!(buf, |s: i32| s as f32 / i32::MAX as f32),
        AudioBufferRef::S16(buf) => mix!(buf, |s: i16| s as f32 / i16::MAX as f32),
        AudioBufferRef::U8(buf) => mix!(buf, |s: u8| (s as f32 / 127.5) - 1.0),
        AudioBufferRef::S24(buf) => {
            mix!(buf, |s: symphonia::core::sample::i24| s.inner() as f32
                / 8_388_607.0)
        }
        AudioBufferRef::U16(buf) => mix!(buf, |s: u16| (s as f32 / u16::MAX as f32) * 2.0 - 1.0),
        AudioBufferRef::U24(buf) => {
            mix!(buf, |s: symphonia::core::sample::u24| (s.inner() as f32
                / 16_777_215.0)
                * 2.0
                - 1.0)
        }
        AudioBufferRef::U32(buf) => mix!(buf, |s: u32| (s as f32 / u32::MAX as f32) * 2.0 - 1.0),
        AudioBufferRef::S8(buf) => mix!(buf, |s: i8| s as f32 / i8::MAX as f32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a stereo 48 kHz WAV of a 440 Hz tone and decode it back.
    #[test]
    fn decodes_and_resamples_wav() {
        let dir = std::env::temp_dir().join(format!("embral-decode-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tone.wav");

        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let seconds = 2.0f32;
        let frames = (48_000.0 * seconds) as usize;
        {
            let mut writer = hound::WavWriter::create(&path, spec).unwrap();
            for i in 0..frames {
                let t = i as f32 / 48_000.0;
                let s = (t * 440.0 * std::f32::consts::TAU).sin();
                let v = (s * 0.5 * i16::MAX as f32) as i16;
                writer.write_sample(v).unwrap(); // L
                writer.write_sample(v).unwrap(); // R
            }
            writer.finalize().unwrap();
        }

        let pcm = decode_to_pcm16k(&path).unwrap();
        let expected = (16_000.0 * seconds) as usize;
        // Chunked FFT resampling pads the tail; allow one chunk of slack.
        assert!(
            (pcm.len() as i64 - expected as i64).unsigned_abs() < 2048,
            "expected ≈{expected} samples, got {}",
            pcm.len()
        );
        // Signal survived (not silence), and stereo downmix stayed in range.
        let peak = pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.3 && peak <= 1.0, "peak {peak}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real-world check against a bundled demo MP3. Ignored by default:
    /// decoding a full earnings call takes ~a minute; run in verification
    /// passes via `cargo test -p embral-engine -- --ignored`.
    #[test]
    #[ignore = "decodes a full-length demo mp3; run explicitly"]
    fn decodes_repo_demo_mp3() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/prepop/embral-demo/audio");
        let Some(mp3) = std::fs::read_dir(&fixture)
            .ok()
            .and_then(|mut it| it.find_map(|e| e.ok().map(|e| e.path())))
        else {
            eprintln!("no demo mp3 found; skipping");
            return;
        };
        let pcm = decode_to_pcm16k(&mp3).unwrap();
        assert!(pcm.len() > 16_000, "expected at least a second of audio");
    }
}

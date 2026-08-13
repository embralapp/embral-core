//! Post-recording speaker pipeline.
//!
//! Runs inside `finalize_meeting` before the transcript is formatted:
//! diarize the full recording, then write display labels and registry links
//! onto the transcript segments. User-given live names claim their
//! clusters; everything else gets "Speaker N" in order of first appearance.
//! The segment-mapping math is pure and lives in `embral_engine::speakers`;
//! this module is the glue that owns audio reading and the labeling policy.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use embral_db::Db;
use embral_engine::speakers as math;
use embral_engine::Engine;
use embral_types::{AppConfig, TranscriptionSegment};

#[cfg(test)]
const SAMPLE_RATE: f64 = 16_000.0;

/// Read a 16 kHz mono WAV (the recorder's own format, f32 or i16) back into
/// samples for diarization.
pub fn read_wav_16k(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("open {}", path.display()))?;
    let spec = reader.spec();
    anyhow::ensure!(
        spec.sample_rate == 16_000 && spec.channels == 1,
        "expected 16 kHz mono, got {} Hz / {} ch",
        spec.sample_rate,
        spec.channels
    );
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<std::result::Result<Vec<_>, _>>()?,
    };
    Ok(samples)
}

/// Run the pipeline, labeling `segments` in place (overwriting any
/// provisional live labels). CPU-heavy; call from a blocking context. Any
/// error leaves the segments as they came in (the meeting still finishes,
/// keeping live labels when they exist).
pub fn run(
    engine: &Engine,
    db: &Db,
    config: &AppConfig,
    samples: &[f32],
    segments: &mut [TranscriptionSegment],
) -> Result<()> {
    let started = std::time::Instant::now();
    let spans = engine.diarize(
        samples,
        config.diarization_sensitivity.clustering_threshold(),
    )?;
    if spans.is_empty() {
        tracing::info!("diarization found no speech turns; leaving segments unlabeled");
        return Ok(());
    }

    // Clusters in order of first appearance; that order drives numbering.
    let mut clusters: Vec<usize> = Vec::new();
    for s in &spans {
        if !clusters.contains(&s.cluster) {
            clusters.push(s.cluster);
        }
    }

    // Map segments onto clusters up front: it drives the final label write
    // and lets user-given live names (renamed pills during the recording)
    // claim the clusters their segments cover; an explicit name outranks
    // anything this pass could infer.
    let times: Vec<(f64, f64)> = segments.iter().map(|s| (s.start, s.end)).collect();
    let seg_clusters = math::label_segments(&times, &spans);
    let user_labels = dominant_user_labels(segments, &seg_clusters);
    let profile_id_by_name: HashMap<String, String> = if user_labels.is_empty() {
        HashMap::new()
    } else {
        db.list_speakers()?
            .into_iter()
            .map(|p| (p.name.to_lowercase(), p.id))
            .collect()
    };

    let mut assignments: HashMap<usize, (String, Option<String>)> = HashMap::new();
    let mut next_number = 1usize;
    for &cluster in &clusters {
        // A user named this cluster live: keep the name (linked to its
        // profile when one matches), no numbered label.
        if let Some(name) = user_labels.get(&cluster) {
            let id = profile_id_by_name.get(&name.to_lowercase()).cloned();
            assignments.insert(cluster, (name.clone(), id));
            continue;
        }
        assignments.insert(cluster, (format!("Speaker {next_number}"), None));
        next_number += 1;
    }

    // --- Label the transcript ----------------------------------------------
    // Wholesale: this pass is the authority, so any provisional live labels
    // the session produced are overwritten (or cleared where diarization
    // found no overlapping speech); user-given names were already merged
    // into `assignments` above.
    for (seg, cluster) in segments.iter_mut().zip(seg_clusters.iter().copied()) {
        let assigned = cluster.and_then(|c| assignments.get(&c));
        seg.speaker = assigned.map(|(name, _)| name.clone());
        seg.speaker_id = assigned.and_then(|(_, id)| id.clone());
    }

    tracing::info!(
        clusters = clusters.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "speaker pipeline finished"
    );
    Ok(())
}

/// A session-generated numbered label ("Speaker 3") as opposed to a name a
/// user typed over a pill.
fn is_generic_label(label: &str) -> bool {
    label
        .strip_prefix("Speaker ")
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// The user-given name for each diarized cluster, if any: among a cluster's
/// segments, the most frequent incoming non-generic label. Incoming labels
/// are the session's live labels after any pill renames; generic
/// "Speaker N" labels are machine guesses and carry no vote.
fn dominant_user_labels(
    segments: &[TranscriptionSegment],
    seg_clusters: &[Option<usize>],
) -> HashMap<usize, String> {
    let mut votes: HashMap<usize, HashMap<&str, usize>> = HashMap::new();
    for (seg, cluster) in segments.iter().zip(seg_clusters.iter().copied()) {
        if let (Some(cluster), Some(label)) = (cluster, seg.speaker.as_deref()) {
            if !is_generic_label(label) {
                *votes.entry(cluster).or_default().entry(label).or_default() += 1;
            }
        }
    }
    votes
        .into_iter()
        .filter_map(|(cluster, counts)| {
            counts
                .into_iter()
                .max_by_key(|(_, n)| *n)
                .map(|(label, _)| (cluster, label.to_string()))
        })
        .collect()
}

/// Full-pipeline e2e over real weights: set `EMBRAL_TEST_DIARIZE_WAV` to a
/// two-speaker 16 kHz WAV (speaker A first) and download `speaker-id`, then
/// `cargo test -p embral --lib speaker_pipeline -- --ignored --nocapture`.
#[cfg(test)]
mod tests {
    use super::*;
    use embral_types::TranscriptionSegment;

    #[test]
    fn user_labels_outvote_generics_per_cluster() {
        let seg = |speaker: Option<&str>| TranscriptionSegment {
            speaker: speaker.map(String::from),
            speaker_id: None,
            text: "hi".into(),
            start: 0.0,
            end: 1.0,
        };
        let segments = vec![
            seg(Some("Speaker 1")), // machine guess: no vote
            seg(Some("Avirut")),
            seg(Some("Avirut")),
            seg(Some("Speaker 2")),
            seg(None),
        ];
        let clusters = vec![Some(0), Some(0), Some(0), Some(1), None];
        let labels = dominant_user_labels(&segments, &clusters);
        assert_eq!(labels.get(&0).map(String::as_str), Some("Avirut"));
        assert!(!labels.contains_key(&1), "generic labels carry no vote");

        assert!(is_generic_label("Speaker 12"));
        assert!(!is_generic_label("Speaker"));
        assert!(!is_generic_label("Speaker Twelve"));
        assert!(!is_generic_label("Sam"));
    }

    fn seg(start: f64, end: f64) -> TranscriptionSegment {
        TranscriptionSegment {
            speaker: None,
            speaker_id: None,
            text: "words".into(),
            start,
            end,
        }
    }

    #[test]
    #[ignore = "requires the speaker-id model and EMBRAL_TEST_DIARIZE_WAV"]
    fn speaker_pipeline_labels_clusters() {
        let wav = std::env::var("EMBRAL_TEST_DIARIZE_WAV").expect("set EMBRAL_TEST_DIARIZE_WAV");
        let samples = read_wav_16k(Path::new(&wav)).expect("read wav");
        let engine = Engine::new();
        let db = Db::open_in_memory().unwrap();
        let config = AppConfig::default();

        // Segments roughly matching the synth turns (30 s file, alternating).
        let total = samples.len() as f64 / SAMPLE_RATE;
        let step = total / 6.0;
        let mut segments: Vec<TranscriptionSegment> = (0..6)
            .map(|i| seg(i as f64 * step + 0.2, (i + 1) as f64 * step - 0.2))
            .collect();

        run(&engine, &db, &config, &samples, &mut segments).expect("pipeline");
        let labels: Vec<_> = segments.iter().filter_map(|s| s.speaker.clone()).collect();
        assert_eq!(labels.len(), 6, "every segment labeled");
        assert!(labels.contains(&"Speaker 1".to_string()));
        assert!(labels.contains(&"Speaker 2".to_string()));

        // A live rename survives the pass: pre-label the first segment with a
        // user-given name and re-run; its whole cluster keeps the name.
        let mut segments2: Vec<TranscriptionSegment> = (0..6)
            .map(|i| seg(i as f64 * step + 0.2, (i + 1) as f64 * step - 0.2))
            .collect();
        segments2[0].speaker = Some("David".into());
        run(&engine, &db, &config, &samples, &mut segments2).expect("pipeline");
        assert_eq!(segments2[0].speaker.as_deref(), Some("David"));
    }
}

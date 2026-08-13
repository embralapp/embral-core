//! One live transcription session over a warm recognizer.
//!
//! Synchronous by design: the caller runs it on a blocking thread and feeds
//! 16 kHz mono f32 chunks (the recorder's native output). Events come back in
//! the same call, mirroring the pull model the old Parakeet session used.
//!
//! Two modes behind one public type:
//!
//! - **Streaming** (Zipformer): the recognizer's built-in endpoint
//!   rules finalize utterances from trailing silence. There is deliberately
//!   no VAD gating here: the endpoint rules need to see the silence,
//!   and streaming encoders are cheap enough to run on it.
//! - **Offline** (Parakeet TDT): Silero VAD segments speech; each completed
//!   segment is decoded whole for the finalized text (full-context accuracy),
//!   with periodic partial decodes of the in-flight segment providing the
//!   live preview.

use std::sync::Arc;

use sherpa_onnx::{
    OfflineRecognizer, OnlinePunctuation, OnlineRecognizer, OnlineStream, RecognizerResult,
    SpeakerEmbeddingExtractor, VoiceActivityDetector,
};

use crate::speakers::OnlineClusterer;

const SAMPLE_RATE: i32 = 16_000;
/// Approximate tail of the last token, added when clamping segment end times.
const LAST_TOKEN_PAD_SECS: f64 = 0.24;
/// Offline mode: re-decode the in-flight segment for an interim after this
/// much new audio has accumulated.
const OFFLINE_INTERIM_STRIDE_SAMPLES: usize = 2 * 16_000;
/// Offline mode: stop refreshing interims for segments longer than this (the
/// VAD's max_speech_duration force-splits shortly after anyway).
const OFFLINE_INTERIM_MAX_SAMPLES: usize = 20 * 16_000;
/// Offline mode: rolling audio kept while the VAD is idle, seeded into the
/// live-preview buffer when speech is confirmed. Silero takes a few hundred
/// ms to flip `detected()`, and the syllables spoken in that window are in
/// the VAD's own segment (finals are complete) but would be missing from
/// every interim: the "first words appear only when the segment finalizes"
/// bug. A little leading silence in the preview decode is harmless.
const OFFLINE_PREROLL_SAMPLES: usize = 16_000;
/// Utterances shorter than this get no live speaker label: a sub-second
/// "yeah" embeds too noisily to cluster honestly. They stay unlabeled; the
/// post-meeting pass places them by temporal overlap.
const MIN_LABEL_SAMPLES: usize = 16_000;

/// Events produced while feeding audio.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// Still-changing preview of the current utterance. Never carries a
    /// speaker; live labeling happens when the utterance completes.
    ///
    /// `text` is the portion that agreed with the previous decode of the
    /// same utterance; `tentative` is the changed suffix, still likely to
    /// move. This is heuristic finality (the local engines expose none per
    /// token; every decode may rewrite anything), split so the UI can dim
    /// the wobbling tail the same way the cloud provider's real one is.
    /// The tail always carries a leading space (the word-boundary
    /// convention at the join between stable text and tail: this diff
    /// splits on words, so the tail always opens one), even when `text` is
    /// empty: the finalized segment before the interim still needs the
    /// separator.
    Interim {
        text: String,
        tentative: Option<String>,
        start: f64,
        end: f64,
    },
    /// Finalized utterance (endpoint fired or session finished). `speaker`
    /// is a provisional live label ("Speaker 1/2/…") when live labeling is
    /// active, else `None`.
    Final {
        text: String,
        start: f64,
        end: f64,
        speaker: Option<String>,
    },
}

/// Per-utterance live speaker labeling: embed each completed VAD segment and
/// assign it with one-pass clustering. Labels are a provisional preview:
/// the post-meeting pipeline re-diarizes the whole recording and overwrites
/// them (see `docs/speakers.md`).
pub(crate) struct LiveLabeler {
    embedder: Arc<SpeakerEmbeddingExtractor>,
    clusterer: OnlineClusterer,
}

impl LiveLabeler {
    pub(crate) fn new(embedder: Arc<SpeakerEmbeddingExtractor>) -> Self {
        Self {
            embedder,
            clusterer: OnlineClusterer::new(crate::speakers::ONLINE_CLUSTER_THRESHOLD),
        }
    }

    /// Label one completed utterance, or `None` when it's too short or the
    /// embedding fails (degrade to unlabeled, never to a wrong guess).
    fn label(&mut self, samples: &[f32]) -> Option<String> {
        if samples.len() < MIN_LABEL_SAMPLES {
            return None;
        }
        let stream = self.embedder.create_stream()?;
        stream.accept_waveform(SAMPLE_RATE, samples);
        stream.input_finished();
        let embedding = self.embedder.compute(&stream)?;
        let cluster = self.clusterer.assign(&embedding);
        Some(format!("Speaker {}", cluster + 1))
    }
}

pub struct LocalSession {
    inner: Inner,
}

enum Inner {
    Streaming(StreamingInner),
    Offline(OfflineInner),
}

impl LocalSession {
    pub(crate) fn streaming(
        recognizer: Arc<OnlineRecognizer>,
        stream: OnlineStream,
        punct: Option<Arc<OnlinePunctuation>>,
        native_text: bool,
    ) -> Self {
        LocalSession {
            inner: Inner::Streaming(StreamingInner {
                recognizer,
                stream,
                punct,
                native_text,
                samples_fed: 0,
                seg_base_secs: 0.0,
                last_interim: String::new(),
            }),
        }
    }

    pub(crate) fn offline(
        recognizer: Arc<OfflineRecognizer>,
        vad: VoiceActivityDetector,
        hotword_lines: String,
        native_text: bool,
        labeler: Option<LiveLabeler>,
    ) -> Self {
        LocalSession {
            inner: Inner::Offline(OfflineInner {
                recognizer,
                vad,
                hotword_lines,
                native_text,
                labeler,
                samples_fed: 0,
                live_buf: Vec::new(),
                preroll: Vec::new(),
                last_interim_len: 0,
                last_interim: String::new(),
            }),
        }
    }

    /// Feed one chunk of 16 kHz mono samples; returns any events it produced.
    pub fn accept(&mut self, pcm: &[f32]) -> Vec<SessionEvent> {
        match &mut self.inner {
            Inner::Streaming(s) => s.accept(pcm),
            Inner::Offline(s) => s.accept(pcm),
        }
    }

    /// Flush trailing audio and finalize whatever remains.
    pub fn finish(self) -> Vec<SessionEvent> {
        match self.inner {
            Inner::Streaming(s) => s.finish(),
            Inner::Offline(s) => s.finish(),
        }
    }

    /// Force-finalize the in-flight utterance (a starred moment): the words
    /// spoken after it start a new segment, so the marker falls exactly
    /// between what came before and after. A no-op mid-silence.
    pub fn split_now(&mut self) -> Vec<SessionEvent> {
        match &mut self.inner {
            Inner::Streaming(s) => s.split_now(),
            Inner::Offline(s) => s.split_now(),
        }
    }

    /// The stream clock: seconds of audio accepted so far. This is the
    /// timeline segments are stamped in (the wall clock runs ahead of it
    /// by the processing backlog), so timestamps that must order against
    /// segments (starred moments) are taken from here.
    pub fn stream_secs(&self) -> f64 {
        match &self.inner {
            Inner::Streaming(s) => s.samples_fed as f64 / SAMPLE_RATE as f64,
            Inner::Offline(s) => s.samples_fed as f64 / SAMPLE_RATE as f64,
        }
    }
}

// --- Streaming mode -------------------------------------------------------

struct StreamingInner {
    recognizer: Arc<OnlineRecognizer>,
    stream: OnlineStream,
    punct: Option<Arc<OnlinePunctuation>>,
    /// True for models that already emit punctuated, cased prose (NeMo
    /// family): their text passes through untouched instead of the
    /// lowercase → punctuate/naive-case pipeline used for icefall models.
    native_text: bool,
    samples_fed: u64,
    /// Stream-absolute second at which the current utterance window began
    /// (i.e. the time of the previous endpoint). Fallback timing base when
    /// the result carries no `start_time`.
    seg_base_secs: f64,
    last_interim: String,
}

impl StreamingInner {
    fn now_secs(&self) -> f64 {
        self.samples_fed as f64 / SAMPLE_RATE as f64
    }

    fn accept(&mut self, pcm: &[f32]) -> Vec<SessionEvent> {
        self.stream.accept_waveform(SAMPLE_RATE, pcm);
        self.samples_fed += pcm.len() as u64;
        self.drain_ready();

        let mut events = Vec::new();
        let now = self.now_secs();

        if self.recognizer.is_endpoint(&self.stream) {
            if let Some(ev) = self.take_final(now) {
                events.push(ev);
            }
            // Reset endpoint state for the next utterance even if the segment
            // was empty (endpoint can fire on pure silence).
            self.recognizer.reset(&self.stream);
            self.seg_base_secs = now;
            self.last_interim.clear();
        } else if let Some(res) = self.recognizer.get_result(&self.stream) {
            let raw = res.text.trim();
            let display = if self.native_text {
                raw.to_string()
            } else {
                // Interims are a live preview; cheap lowercase is enough
                // (full punctuation/casing happens on finalization).
                raw.to_lowercase()
            };
            if !display.is_empty() && display != self.last_interim {
                let (text, tentative) = split_agreed(&self.last_interim, &display);
                self.last_interim = display;
                let (start, end) = self.segment_times(&res, now);
                events.push(SessionEvent::Interim {
                    text,
                    tentative,
                    start,
                    end,
                });
            }
        }
        events
    }

    fn finish(mut self) -> Vec<SessionEvent> {
        self.stream.input_finished();
        self.drain_ready();
        let now = self.now_secs();
        self.take_final(now).into_iter().collect()
    }

    /// Finalize the current hypothesis mid-stream (same motions as an
    /// endpoint firing).
    fn split_now(&mut self) -> Vec<SessionEvent> {
        self.drain_ready();
        let now = self.now_secs();
        let ev = self.take_final(now);
        self.recognizer.reset(&self.stream);
        self.seg_base_secs = now;
        self.last_interim.clear();
        ev.into_iter().collect()
    }

    fn drain_ready(&mut self) {
        while self.recognizer.is_ready(&self.stream) {
            self.recognizer.decode(&self.stream);
        }
    }

    /// Pull the current hypothesis as a finalized event, if non-empty.
    fn take_final(&mut self, now: f64) -> Option<SessionEvent> {
        let res = self.recognizer.get_result(&self.stream)?;
        let raw = res.text.trim().to_string();
        if raw.is_empty() {
            return None;
        }
        let (start, end) = self.segment_times(&res, now);
        let text = if self.native_text {
            raw
        } else {
            polish(&raw, self.punct.as_deref())
        };
        // Streaming mode never labels: it doesn't hold the utterance audio.
        Some(SessionEvent::Final {
            text,
            start,
            end,
            speaker: None,
        })
    }

    /// Derive stream-absolute (start, end) for the current utterance from the
    /// result's own timing when present, else from our sample cursor.
    fn segment_times(&self, res: &RecognizerResult, now: f64) -> (f64, f64) {
        let base = res
            .start_time
            .map(|t| t as f64)
            .unwrap_or(self.seg_base_secs);
        match res.timestamps.as_ref().filter(|t| !t.is_empty()) {
            Some(ts) => {
                let start = base + ts[0] as f64;
                let end = (base + ts[ts.len() - 1] as f64 + LAST_TOKEN_PAD_SECS).min(now);
                (start, end.max(start))
            }
            None => (base, now.max(base)),
        }
    }
}

// --- Offline (VAD-segmented) mode ------------------------------------------

struct OfflineInner {
    recognizer: Arc<OfflineRecognizer>,
    vad: VoiceActivityDetector,
    /// Pre-encoded hotword lines; empty = plain streams.
    hotword_lines: String,
    native_text: bool,
    /// Live speaker labeling over completed VAD segments, when enabled.
    labeler: Option<LiveLabeler>,
    samples_fed: u64,
    /// Samples of the in-flight (still-detected) speech run, for interims.
    live_buf: Vec<f32>,
    /// The last ~1 s of audio heard while the VAD was idle: the words that
    /// were already being spoken when detection confirms.
    preroll: Vec<f32>,
    /// `live_buf` length at the last interim decode.
    last_interim_len: usize,
    last_interim: String,
}

impl OfflineInner {
    fn accept(&mut self, pcm: &[f32]) -> Vec<SessionEvent> {
        self.vad.accept_waveform(pcm);
        self.samples_fed += pcm.len() as u64;

        let mut events = Vec::new();
        self.drain_completed(&mut events);

        if self.vad.detected() {
            if self.live_buf.is_empty() {
                // Detection just confirmed: the first syllables are in the
                // preroll, not in this chunk.
                std::mem::swap(&mut self.live_buf, &mut self.preroll);
            }
            self.live_buf.extend_from_slice(pcm);
            let grew = self.live_buf.len().saturating_sub(self.last_interim_len);
            if grew >= OFFLINE_INTERIM_STRIDE_SAMPLES
                && self.live_buf.len() <= OFFLINE_INTERIM_MAX_SAMPLES
            {
                self.last_interim_len = self.live_buf.len();
                let decoded = self.decode(&self.live_buf);
                if !decoded.is_empty() && decoded != self.last_interim {
                    let (text, tentative) = split_agreed(&self.last_interim, &decoded);
                    self.last_interim = decoded;
                    let end = self.samples_fed as f64 / SAMPLE_RATE as f64;
                    let start = end - self.live_buf.len() as f64 / SAMPLE_RATE as f64;
                    events.push(SessionEvent::Interim {
                        text,
                        tentative,
                        start: start.max(0.0),
                        end,
                    });
                }
            }
        } else {
            roll(&mut self.preroll, pcm, OFFLINE_PREROLL_SAMPLES);
            if !self.live_buf.is_empty() {
                // Speech run ended; the completed segment arrives via the
                // VAD queue (drained above / next tick).
                self.reset_live();
            }
        }
        events
    }

    fn finish(mut self) -> Vec<SessionEvent> {
        self.vad.flush();
        let mut events = Vec::new();
        self.drain_completed(&mut events);
        events
    }

    /// Force-close the in-flight VAD segment so it finalizes now; the VAD
    /// keeps accepting audio afterwards (the next words open a new
    /// segment).
    fn split_now(&mut self) -> Vec<SessionEvent> {
        self.vad.flush();
        let mut events = Vec::new();
        self.drain_completed(&mut events);
        events
    }

    /// Decode every completed VAD segment into a Final event, live-labeling
    /// the speaker while the segment's samples are still in hand.
    fn drain_completed(&mut self, events: &mut Vec<SessionEvent>) {
        while let Some(segment) = self.vad.front() {
            let start = segment.start() as f64 / SAMPLE_RATE as f64;
            let end = start + segment.n() as f64 / SAMPLE_RATE as f64;
            let text = self.decode(segment.samples());
            let speaker = if text.is_empty() {
                None
            } else {
                self.labeler.as_mut().and_then(|l| l.label(segment.samples()))
            };
            self.vad.pop();
            self.reset_live();
            if !text.is_empty() {
                events.push(SessionEvent::Final {
                    text,
                    start,
                    end,
                    speaker,
                });
            }
        }
    }

    fn reset_live(&mut self) {
        self.live_buf.clear();
        self.last_interim_len = 0;
        self.last_interim.clear();
    }

    /// One-shot offline decode of a sample buffer.
    fn decode(&self, samples: &[f32]) -> String {
        let stream = if self.hotword_lines.is_empty() {
            self.recognizer.create_stream()
        } else {
            self.recognizer
                .create_stream_with_hotwords(&self.hotword_lines)
        };
        stream.accept_waveform(SAMPLE_RATE, samples);
        self.recognizer.decode(&stream);
        let raw = stream
            .get_result()
            .map(|r| r.text.trim().to_string())
            .unwrap_or_default();
        if raw.is_empty() {
            return raw;
        }
        if self.native_text {
            raw
        } else {
            naive_case(&raw.to_lowercase())
        }
    }
}

/// Split the current interim decode against the previous one: the longest
/// common word prefix is the stable part, the changed suffix the tentative
/// tail. Word granularity on purpose: a character-level split would carve
/// syllables the decoder never produced. Because the split is on words, the
/// tail always opens a new word, so it always carries the leading space at
/// the join with the stable text, even when the stable part is empty, since
/// whatever precedes the interim (a finalized segment) still needs the
/// separator; renderers collapse the space when nothing does.
fn split_agreed(prev: &str, curr: &str) -> (String, Option<String>) {
    let prev_words: Vec<&str> = prev.split_whitespace().collect();
    let curr_words: Vec<&str> = curr.split_whitespace().collect();
    let agreed = prev_words
        .iter()
        .zip(&curr_words)
        .take_while(|(a, b)| a == b)
        .count();
    if agreed == curr_words.len() {
        // The whole decode already agreed (e.g. the previous one was
        // longer and shrank): everything is stable.
        return (curr_words.join(" "), None);
    }
    let stable = curr_words[..agreed].join(" ");
    let tail = format!(" {}", curr_words[agreed..].join(" "));
    (stable, Some(tail))
}

/// Append `pcm` to a rolling buffer, keeping only the newest `cap` samples.
fn roll(buf: &mut Vec<f32>, pcm: &[f32], cap: usize) {
    buf.extend_from_slice(pcm);
    let excess = buf.len().saturating_sub(cap);
    if excess > 0 {
        buf.drain(..excess);
    }
}

// --- Shared text polish ----------------------------------------------------

/// Turn the model's raw ALL-CAPS unpunctuated hypothesis into readable prose:
/// through the punctuation model when available, else naive sentence casing.
fn polish(raw: &str, punct: Option<&OnlinePunctuation>) -> String {
    let lower = raw.trim().to_lowercase();
    if let Some(p) = punct {
        if let Some(out) = p.add_punctuation(&lower) {
            let out = out.trim();
            if !out.is_empty() {
                return out.to_string();
            }
        }
        tracing::warn!("punctuation model returned no output; using naive casing");
    }
    naive_case(&lower)
}

/// Capitalize the first letter and the standalone pronoun "i" (incl. "i'm",
/// "i'll", …). No punctuation is invented; that is the punct model's job.
fn naive_case(lower: &str) -> String {
    let mut words: Vec<String> = lower.split_whitespace().map(String::from).collect();
    for w in words.iter_mut() {
        if w == "i" || w.starts_with("i'") {
            w.replace_range(0..1, "I");
        }
    }
    let mut out = words.join(" ");
    if let Some(first) = out.chars().next() {
        if first.is_ascii_lowercase() {
            out.replace_range(0..1, &first.to_ascii_uppercase().to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_agreed_hardens_the_shared_prefix() {
        // First decode of an utterance: everything is tentative. The tail
        // still leads with a space; a finalized segment may precede it.
        assert_eq!(
            split_agreed("", "the quick"),
            (String::new(), Some(" the quick".to_string()))
        );
        // The agreeing prefix is stable; the changed suffix (with its
        // word-boundary leading space) is tentative.
        assert_eq!(
            split_agreed("the quick brown fex", "the quick brown fox jumps"),
            (
                "the quick brown".to_string(),
                Some(" fox jumps".to_string())
            )
        );
        // Full agreement (a shrunken re-decode): all stable.
        assert_eq!(
            split_agreed("the quick brown", "the quick"),
            ("the quick".to_string(), None)
        );
        // A revision at the very start un-hardens everything.
        assert_eq!(
            split_agreed("he quick", "the quick"),
            (String::new(), Some(" the quick".to_string()))
        );
    }

    #[test]
    fn roll_keeps_only_the_newest_samples() {
        let mut buf = vec![1.0, 2.0];
        roll(&mut buf, &[3.0, 4.0], 3);
        assert_eq!(buf, vec![2.0, 3.0, 4.0]);
        // Under the cap: nothing trimmed.
        let mut small = vec![1.0];
        roll(&mut small, &[2.0], 8);
        assert_eq!(small, vec![1.0, 2.0]);
        // One oversized chunk keeps its own tail.
        let mut empty = Vec::new();
        roll(&mut empty, &[1.0, 2.0, 3.0, 4.0], 2);
        assert_eq!(empty, vec![3.0, 4.0]);
    }

    #[test]
    fn naive_case_capitalizes_sentence_and_pronoun() {
        assert_eq!(
            naive_case("i think i'm ready and i'll go"),
            "I think I'm ready and I'll go"
        );
        assert_eq!(naive_case("hello there"), "Hello there");
        assert_eq!(naive_case(""), "");
    }

    #[test]
    fn polish_without_punct_model_uses_naive_casing() {
        assert_eq!(polish("  HELLO THERE I SAID  ", None), "Hello there I said");
    }
}

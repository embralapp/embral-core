//! The silence check-in's decision rule ([detection.md] §Auto-stop on
//! silence): after a configured stretch with no transcribed word, the
//! recording asks "Still recording?"; unanswered past a fixed grace it
//! stops (or a notice is shown, per the setting). Pure logic; the watcher task in
//! `commands::recording` feeds it clocks and acts on the verdict.

/// How long an unanswered check-in waits before acting. A constant, not a
/// setting: long enough to grab the mouse mid-meeting, short enough to
/// still save the hours the feature exists to save.
pub const GRACE_SECS: u64 = 120;

/// Liveness evidence from the transcription stream, kept by the event
/// forwarder. The check-in must count words as they arrive on screen, not
/// only utterances that close; a segment can stay open for minutes of
/// live speech. Interims carry the in-flight utterance's committed (final)
/// text beside a tentative tail; only the committed part counts, because a
/// tentative hypothesis can be noise that never becomes a word.
#[derive(Debug, Default)]
pub struct LivenessTracker {
    /// The committed text of the last interim seen. Empty at rest and
    /// after every segment close; an utterance starts from nothing.
    last_committed: String,
}

impl LivenessTracker {
    /// A live preview arrived; true when it proves new final tokens:
    /// committed text that is non-empty and not what it was. Growth is the
    /// common case; any revision still shows live decoding. Empty never
    /// counts: after a close, tentative-only flicker arrives with no
    /// committed text at all.
    pub fn observe_interim(&mut self, committed: &str) -> bool {
        let advanced = !committed.is_empty() && committed != self.last_committed;
        if committed != self.last_committed {
            self.last_committed.clear();
            self.last_committed.push_str(committed);
        }
        advanced
    }

    /// An utterance closed: always liveness (final words landed), and the
    /// next utterance's committed text starts from nothing again.
    pub fn observe_segment(&mut self) -> bool {
        self.last_committed.clear();
        true
    }
}

/// The check-in's current standing, as the watcher tracks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// No check-in showing.
    None,
    /// A check-in fired this long ago and is awaiting an answer.
    Pending { age_secs: u64 },
    /// "Keep recording" ran its course — no re-nagging until speech resumes.
    StoodDown,
}

/// What one watcher tick should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing to do.
    Quiet,
    /// Silence just crossed the threshold: raise the check-in.
    Notify,
    /// A check-in is up; keep waiting for an answer.
    Waiting,
    /// The grace ran out with no answer: act per the setting.
    Unanswered,
    /// Speech resumed: take the check-in (or the stand-down) back down.
    Cleared,
}

pub fn check(silence_secs: u64, threshold_secs: u64, notice: Notice) -> Verdict {
    if threshold_secs == 0 {
        // Off. The caller also skips entirely; this keeps the rule total.
        return match notice {
            Notice::None => Verdict::Quiet,
            _ => Verdict::Cleared,
        };
    }
    if silence_secs < threshold_secs {
        return match notice {
            Notice::None => Verdict::Quiet,
            _ => Verdict::Cleared,
        };
    }
    match notice {
        Notice::None => Verdict::Notify,
        Notice::Pending { age_secs } if age_secs >= GRACE_SECS => Verdict::Unanswered,
        Notice::Pending { .. } => Verdict::Waiting,
        Notice::StoodDown => Verdict::Quiet,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THRESHOLD: u64 = 300; // the 5-minute default

    #[test]
    fn quiet_until_the_threshold_then_notify_once() {
        assert_eq!(check(0, THRESHOLD, Notice::None), Verdict::Quiet);
        assert_eq!(check(299, THRESHOLD, Notice::None), Verdict::Quiet);
        assert_eq!(check(300, THRESHOLD, Notice::None), Verdict::Notify);
        // With the notice up, later ticks wait rather than re-notify.
        assert_eq!(
            check(360, THRESHOLD, Notice::Pending { age_secs: 60 }),
            Verdict::Waiting
        );
    }

    #[test]
    fn the_grace_running_out_means_unanswered() {
        assert_eq!(
            check(420, THRESHOLD, Notice::Pending { age_secs: GRACE_SECS }),
            Verdict::Unanswered
        );
    }

    #[test]
    fn speech_resuming_clears_a_notice_or_a_stand_down() {
        assert_eq!(
            check(10, THRESHOLD, Notice::Pending { age_secs: 60 }),
            Verdict::Cleared
        );
        assert_eq!(check(10, THRESHOLD, Notice::StoodDown), Verdict::Cleared);
        assert_eq!(check(10, THRESHOLD, Notice::None), Verdict::Quiet);
    }

    #[test]
    fn a_stand_down_never_renags_while_silence_continues() {
        assert_eq!(check(9999, THRESHOLD, Notice::StoodDown), Verdict::Quiet);
    }

    #[test]
    fn zero_threshold_is_off() {
        assert_eq!(check(9999, 0, Notice::None), Verdict::Quiet);
        // A notice left up when the setting turns off comes down.
        assert_eq!(
            check(9999, 0, Notice::Pending { age_secs: 10 }),
            Verdict::Cleared
        );
    }

    #[test]
    fn final_tokens_arriving_are_liveness() {
        let mut t = LivenessTracker::default();
        assert!(t.observe_interim("hello"));
        assert!(t.observe_interim("hello world"));
    }

    #[test]
    fn tentative_only_flicker_is_not() {
        // The same committed text again means only the tentative tail
        // moved: a hypothesis, not a word.
        let mut t = LivenessTracker::default();
        assert!(t.observe_interim("hello"));
        assert!(!t.observe_interim("hello"));
    }

    #[test]
    fn an_utterance_opens_from_nothing_without_counting() {
        // Both providers' first interim of an utterance can carry no
        // committed text yet (local always, cloud on a tentative-only
        // response); nothing final has arrived.
        let mut t = LivenessTracker::default();
        assert!(!t.observe_interim(""));
    }

    #[test]
    fn noise_after_a_close_is_not_liveness() {
        // After a segment closes, tentative-only interims arrive with
        // empty committed text; they must not keep a dead room alive.
        let mut t = LivenessTracker::default();
        assert!(t.observe_interim("see you tomorrow"));
        assert!(t.observe_segment());
        assert!(!t.observe_interim(""));
        assert!(!t.observe_interim(""));
    }

    #[test]
    fn a_close_resets_so_the_next_words_count() {
        let mut t = LivenessTracker::default();
        assert!(t.observe_interim("first thought"));
        assert!(t.observe_segment());
        assert!(t.observe_interim("second thought"));
    }

    #[test]
    fn a_revision_still_shows_live_decoding() {
        // The local engine's committed part is the agreed prefix of two
        // consecutive decodes; it can shrink while words keep arriving.
        let mut t = LivenessTracker::default();
        assert!(t.observe_interim("hello world"));
        assert!(t.observe_interim("hello"));
    }
}

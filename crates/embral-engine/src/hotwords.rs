//! Vocabulary boost: encode user phrases into the BPE token sequences the
//! recognizer's hotword decoder expects.
//!
//! The Zipformer English models ship `tokens.txt` (one `PIECE ID` per line,
//! uppercase pieces, `▁` marking a word start) but no `bpe.vocab`, so instead
//! of sherpa's `modeling_unit`/`bpe_vocab` path we tokenize each phrase
//! ourselves by greedy longest-piece matching and pass the result to
//! `create_stream_with_hotwords`: one line of space-separated tokens per
//! phrase, the format sherpa accepts when no modeling unit is configured.
//!
//! Phrases that can't be fully expressed with the model's pieces (digits,
//! non-Latin scripts, symbols) are skipped and reported to the caller.

use std::collections::HashSet;

/// Word-start marker used by sentencepiece BPE vocabularies.
const WORD_START: char = '\u{2581}'; // ▁

pub struct TokenSet {
    pieces: HashSet<String>,
    max_piece_chars: usize,
}

impl TokenSet {
    /// Parse a sherpa `tokens.txt` (lines of `PIECE ID`). Special markers like
    /// `<blk>`, `<sos/eos>`, `<unk>` are excluded.
    pub fn parse(tokens_txt: &str) -> TokenSet {
        let mut pieces = HashSet::new();
        let mut max_piece_chars = 0;
        for line in tokens_txt.lines() {
            let Some(piece) = line.split_whitespace().next() else {
                continue;
            };
            if piece.starts_with('<') && piece.ends_with('>') {
                continue;
            }
            max_piece_chars = max_piece_chars.max(piece.chars().count());
            pieces.insert(piece.to_string());
        }
        TokenSet {
            pieces,
            max_piece_chars,
        }
    }

    /// Encode one phrase to a hotword line (space-separated pieces), or `None`
    /// if any word can't be fully tokenized with this model's pieces.
    ///
    /// Vocabulary casing differs by model family (icefall Zipformers are
    /// UPPERCASE, NeMo BPE vocabs are lowercase), so each word is tried in
    /// both casings and the first that tokenizes fully wins.
    pub fn encode_phrase(&self, phrase: &str) -> Option<String> {
        let mut out: Vec<String> = Vec::new();
        for word in phrase.split_whitespace() {
            let pieces = self
                .encode_word(&word.to_uppercase())
                .or_else(|| self.encode_word(&word.to_lowercase()))?;
            out.extend(pieces);
        }
        if out.is_empty() {
            None
        } else {
            Some(out.join(" "))
        }
    }

    /// Greedy longest-piece tokenization of one word (already cased).
    fn encode_word(&self, word: &str) -> Option<Vec<String>> {
        let target: Vec<char> = std::iter::once(WORD_START).chain(word.chars()).collect();
        let mut out = Vec::new();
        let mut pos = 0;
        while pos < target.len() {
            let mut matched = None;
            let max_len = self.max_piece_chars.min(target.len() - pos);
            for len in (1..=max_len).rev() {
                let candidate: String = target[pos..pos + len].iter().collect();
                if self.pieces.contains(&candidate) {
                    matched = Some(candidate);
                    break;
                }
            }
            match matched {
                Some(piece) => {
                    pos += piece.chars().count();
                    out.push(piece);
                }
                None => return None,
            }
        }
        Some(out)
    }
}

/// Encode a vocabulary list against a model's `tokens.txt`. Returns the
/// newline-joined hotword lines plus the phrases that had to be skipped.
pub fn encode_vocabulary(tokens_txt: &str, phrases: &[String]) -> (String, Vec<String>) {
    let set = TokenSet::parse(tokens_txt);
    let mut lines = Vec::new();
    let mut skipped = Vec::new();
    for phrase in phrases {
        if phrase.trim().is_empty() {
            continue;
        }
        match set.encode_phrase(phrase) {
            Some(line) => lines.push(line),
            None => skipped.push(phrase.clone()),
        }
    }
    (lines.join("\n"), skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Realistic subset of the actual zipformer-en tokens.txt (uppercase BPE
    // pieces, ▁ word-start convention).
    const FIXTURE: &str = "\
<blk> 0
<sos/eos> 1
<unk> 2
S 3
▁THE 4
▁A 5
T 6
▁AND 7
ED 8
▁OF 9
▁TO 10
E 11
N 12
K 13
▁EM 14
BR 15
AL 16
ING 17
O 18
R 19
▁ 20
D 21
";

    #[test]
    fn encodes_single_known_word() {
        let set = TokenSet::parse(FIXTURE);
        assert_eq!(set.encode_phrase("the").unwrap(), "▁THE");
        // Case-insensitive input.
        assert_eq!(set.encode_phrase("The").unwrap(), "▁THE");
    }

    #[test]
    fn encodes_multi_piece_word_greedily() {
        let set = TokenSet::parse(FIXTURE);
        // ▁EM + BR + AL
        assert_eq!(set.encode_phrase("embral").unwrap(), "▁EM BR AL");
        // ▁TO + K + E + N + S
        assert_eq!(set.encode_phrase("tokens").unwrap(), "▁TO K E N S");
    }

    #[test]
    fn encodes_multi_word_phrase() {
        let set = TokenSet::parse(FIXTURE);
        assert_eq!(
            set.encode_phrase("the embral").unwrap(),
            "▁THE ▁EM BR AL"
        );
    }

    #[test]
    fn rejects_unencodable_input() {
        let set = TokenSet::parse(FIXTURE);
        assert!(set.encode_phrase("07734").is_none()); // digits not in vocab
        assert!(set.encode_phrase("日本語").is_none());
        assert!(set.encode_phrase("").is_none());
    }

    #[test]
    fn special_markers_are_not_pieces() {
        let set = TokenSet::parse(FIXTURE);
        assert!(set.encode_phrase("<unk>").is_none());
    }

    #[test]
    fn encodes_against_lowercase_vocab() {
        // NeMo-style lowercase BPE vocabulary.
        let set = TokenSet::parse("▁the 0\n▁to 1\nk 2\ne 3\nn 4\ns 5\n");
        assert_eq!(set.encode_phrase("The").unwrap(), "▁the");
        assert_eq!(set.encode_phrase("TOKENS").unwrap(), "▁to k e n s");
    }

    #[test]
    fn vocabulary_splits_encodable_and_skipped() {
        let (lines, skipped) = encode_vocabulary(
            FIXTURE,
            &["embral".to_string(), "42".to_string(), "  ".to_string()],
        );
        assert_eq!(lines, "▁EM BR AL");
        assert_eq!(skipped, vec!["42".to_string()]);
    }
}

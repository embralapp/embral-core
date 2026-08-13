//! Reading text out of a pasted image ([storage.md](../../../../docs/storage.md)
//! §The chunk index); unavailable on Linux.
//!
//! The other two platforms use an engine that ships with the OS
//! (`Windows.Media.Ocr`, Vision), which is what lets this feature exist
//! without downloading or bundling anything. Linux has no in-box equivalent:
//! Tesseract is the obvious candidate and is a multi-megabyte dependency
//! plus per-language data files, which this feature's own rule says not to
//! take on ("so nothing is downloaded or bundled").
//!
//! `Unavailable` is the honest answer and the one the caller wants: it
//! leaves the image pending and stops, because nothing else will fare
//! better ([platform/mod.rs] stub rule, and `Recognized`'s own doc).

use crate::platform::types::Recognized;

/// No OCR engine here. Images stay pending and the caller stops retrying.
pub fn recognize_text(_bytes: &[u8]) -> Recognized {
    Recognized::Unavailable
}

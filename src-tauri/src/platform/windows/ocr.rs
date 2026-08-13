//! Reading the text out of an image, Windows side
//! ([storage.md](../../../../docs/storage.md) §The chunk index).
//!
//! `Windows.Media.Ocr` is in the box on every Windows 10/11 install: no
//! model to download, nothing to bundle or sign. It reads printed text
//! (slides, screenshots, documents) well, and handwriting poorly; that is
//! the platform's ceiling, not a defect in this file.
//!
//! This is WinRT rather than Win32, so it wants a COM apartment on the
//! calling thread (the caller runs us on a `spawn_blocking` worker) and its
//! calls are asynchronous, resolved here with `.join()`; every one of them
//! completes in milliseconds against an in-memory stream.

use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform,
    ColorManagementMode, ExifOrientationMode,
};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

use crate::platform::types::Recognized;
use embral_notes::ocr::OcrLine;

/// Read the text in one image. Bytes rather than a path: the decoder is
/// happy with an in-memory stream, and file IO belongs above the platform
/// layer.
pub fn recognize_text(bytes: &[u8]) -> Recognized {
    // SAFETY: the documented apartment call. A thread already initialized
    // in another mode answers RPC_E_CHANGED_MODE, which is fine; we only
    // need an apartment of some kind, and the existing one serves.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
    }

    let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages() else {
        // No OCR language data for any language the user has installed.
        // Nothing here will succeed until that changes.
        return Recognized::Unavailable;
    };

    match read(&engine, bytes) {
        Ok(lines) => {
            // Geometry first: the engine emits lines top to bottom across
            // the whole image, which interleaves columns; layout puts them
            // back into reading order before the text is flattened.
            let ordered = embral_notes::ocr::layout(&lines);
            let borrowed: Vec<&str> = ordered.iter().map(String::as_str).collect();
            Recognized::Text(embral_notes::ocr::normalize(&borrowed))
        }
        Err(e) => Recognized::Failed(e.message()),
    }
}

/// The decode-and-recognize path, with every WinRT error funnelled into one
/// `Failed`. Split out so `recognize_text` stays a readable summary.
fn read(engine: &OcrEngine, bytes: &[u8]) -> windows_core::Result<Vec<OcrLine>> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.join()?;
    writer.FlushAsync()?.join()?;
    // The writer leaves the cursor at the end; the decoder reads forward.
    writer.DetachStream()?;
    stream.Seek(0)?;

    let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
    let bitmap = match scaled_size(&decoder)? {
        None => decoder.GetSoftwareBitmapAsync()?.join()?,
        Some((width, height)) => {
            // Past the engine's limit the call fails outright rather than
            // degrading, so a large screenshot has to be scaled first;
            // the failure mode this guards against is silent and total.
            tracing::debug!("scaling a {width}x{height} image down for OCR");
            let transform = BitmapTransform::new()?;
            transform.SetScaledWidth(width)?;
            transform.SetScaledHeight(height)?;
            transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
            decoder
                .GetSoftwareBitmapTransformedAsync(
                    BitmapPixelFormat::Bgra8,
                    BitmapAlphaMode::Premultiplied,
                    &transform,
                    ExifOrientationMode::RespectExifOrientation,
                    ColorManagementMode::ColorManageToSRgb,
                )?
                .join()?
        }
    };

    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let mut lines = Vec::new();
    for line in result.Lines()? {
        let text = line.Text()?.to_string_lossy();
        // `OcrLine` exposes no rect of its own; its box is the union of
        // its words' (pixel space, top-left origin, what layout wants).
        let mut x0 = f32::INFINITY;
        let mut y0 = f32::INFINITY;
        let mut x1 = f32::NEG_INFINITY;
        let mut y1 = f32::NEG_INFINITY;
        for word in line.Words()? {
            let r = word.BoundingRect()?;
            x0 = x0.min(r.X);
            y0 = y0.min(r.Y);
            x1 = x1.max(r.X + r.Width);
            y1 = y1.max(r.Y + r.Height);
        }
        // A wordless line keeps its text with a zero box rather than
        // losing it; layout tolerates the degenerate rectangle.
        let (x, y, width, height) = if x0.is_finite() {
            (x0, y0, x1 - x0, y1 - y0)
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };
        lines.push(OcrLine { text, x, y, width, height });
    }
    Ok(lines)
}

/// The size to decode at, or `None` when the image already fits. Scaling
/// keeps the aspect ratio: the engine's limit applies to each dimension.
fn scaled_size(decoder: &BitmapDecoder) -> windows_core::Result<Option<(u32, u32)>> {
    let max = OcrEngine::MaxImageDimension()?;
    let width = decoder.PixelWidth()?;
    let height = decoder.PixelHeight()?;
    let longest = width.max(height);
    if longest <= max || longest == 0 {
        return Ok(None);
    }
    let scale = max as f64 / longest as f64;
    Ok(Some((
        ((width as f64 * scale) as u32).max(1),
        ((height as f64 * scale) as u32).max(1),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the engine actually does with a real image, run by hand:
    /// `EMBRAL_TEST_OCR_IMAGE=C:/path/to/slide.png
    ///  cargo test -p embral --lib ocr -- --ignored --nocapture`
    ///
    /// Point it at a screenshot of slides and also at a photographed
    /// whiteboard: printed text and handwriting are different problems and
    /// this engine is much better at the first.
    #[test]
    #[ignore = "needs EMBRAL_TEST_OCR_IMAGE pointing at an image file"]
    fn real_image_is_read() {
        let path = std::env::var("EMBRAL_TEST_OCR_IMAGE").expect("set EMBRAL_TEST_OCR_IMAGE");
        let bytes = std::fs::read(&path).expect("read the image");
        let started = std::time::Instant::now();
        let outcome = recognize_text(&bytes);
        eprintln!(
            "{} bytes in {:.0} ms",
            bytes.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );
        match outcome {
            Recognized::Text(text) => {
                eprintln!("--- usable: {} ---\n{text}", embral_notes::ocr::is_usable(&text));
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn a_non_image_fails_rather_than_pretending() {
        // Refusing to decode is `Failed` (an answer about this file), not
        // `Unavailable`, which would leave it pending forever.
        match recognize_text(b"this is not an image") {
            Recognized::Failed(_) => {}
            // A machine with no OCR language pack cannot get that far, and
            // that is a legitimate result on this box too.
            Recognized::Unavailable => {}
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// The committed two-column fixture through the real engine: the whole
    /// point of carrying geometry is that the left column's text comes out
    /// before the right's instead of interleaved straight across both.
    #[test]
    fn columns_are_read_in_order() {
        const FIXTURE: &[u8] = include_bytes!("../../../testdata/two-column.png");
        let text = match recognize_text(FIXTURE) {
            Recognized::Text(text) => text,
            // No language pack on this machine: the ordering itself is
            // covered by the pure tests; this one is about the engine.
            Recognized::Unavailable => return,
            Recognized::Failed(why) => panic!("engine refused the fixture: {why}"),
        };
        let lower = text.to_lowercase();
        let pos = |w: &str| lower.find(w).unwrap_or_else(|| panic!("{w} missing from: {lower}"));
        for left in ["alpha", "bravo", "charlie"] {
            for right in ["delta", "echo", "foxtrot"] {
                assert!(pos(left) < pos(right), "{left} after {right} in: {lower}");
            }
        }
        // The full-width title reads first, above both columns.
        assert!(pos("quarterly") < pos("alpha"), "title after the left column: {lower}");
        assert!(pos("quarterly") < pos("delta"), "title after the right column: {lower}");
    }
}

//! Fetching a meeting's pasted image for an MCP client, within a budget
//! ([integrations.md] §The MCP server). rmcp-free like `queries`: this
//! module takes paths and a database and returns bytes; the server file
//! wraps them in content blocks.
//!
//! The budget exists because stdio clients, not the transport, set the
//! ceiling: the wire has no size cap, but clients choke on multi-MB
//! results and vision models downscale past ~1568px anyway. In-budget
//! files pass through byte-identical; oversized ones are decoded under
//! hard limits, resized, and re-encoded.

use std::path::{Component, Path, PathBuf};

use embral_db::Db;

use crate::store::ToolError;

/// The long-edge cap. Beyond it, vision-capable clients downscale on
/// their own end; sending more is pure wire weight.
const MAX_EDGE: u32 = 1568;
/// Binary budget. Base64 inflates by 4/3, so this is ~1 MB encoded:
/// the practical ceiling observed across stdio clients.
const MAX_BYTES: usize = 768 * 1024;
/// A file bigger than this is refused outright rather than decoded.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// One image, ready for the wire.
pub struct FetchedImage {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
    /// Whether the bytes are a downscaled re-encode rather than the file.
    pub scaled: bool,
    /// The stamped OCR reading, when one exists and is worth reading.
    pub image_text: Option<String>,
}

/// One path component, nothing more. Meeting ids and filenames both come
/// from the model, and either could try to walk out of the assets tree.
/// Two rejections are spelled out because the component parse is platform
/// specific: `:` because `img.png:stream` is one component to the parser
/// and an alternate data stream to NTFS, and `\` because it separates
/// paths only on Windows, so `a\b` would pass as a bare name everywhere
/// else.
fn require_bare_component(name: &str, what: &str) -> Result<(), ToolError> {
    let mut components = Path::new(name).components();
    let sound = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !name.contains(':')
        && !name.contains('\\')
        && !name.contains('\0');
    if sound {
        Ok(())
    } else {
        Err(ToolError::InvalidArgument {
            message: format!("{what} must be a bare name, not a path: {name:?}"),
        })
    }
}

fn mime_for(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// The image filenames on disk for one meeting, sorted: the inventory
/// `get_meeting` reports (the ocr sweep's `stored_images` shape).
pub fn list(storage_dir: &Path, meeting_id: &str) -> Vec<String> {
    if require_bare_component(meeting_id, "meeting id").is_err() {
        return Vec::new();
    }
    let dir = storage_dir.join("assets").join(meeting_id);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

/// Read one image within the budget. The meeting must exist before the
/// filesystem is touched at all; the mime comes from the bytes, never the
/// extension (a junk file in the directory is an expected case, answered
/// as an argument error rather than served as an "image").
pub fn fetch(
    storage_dir: &Path,
    db: &Db,
    meeting_id: &str,
    image: &str,
) -> Result<FetchedImage, ToolError> {
    require_bare_component(meeting_id, "meeting id")?;
    require_bare_component(image, "image")?;
    if db.get_meeting(meeting_id).map_err(ToolError::Db)?.is_none() {
        return Err(ToolError::MeetingNotFound {
            id: meeting_id.to_string(),
        });
    }

    let path: PathBuf = storage_dir.join("assets").join(meeting_id).join(image);
    let size = match std::fs::metadata(&path) {
        Ok(m) if m.is_file() => m.len(),
        _ => {
            return Err(ToolError::ImageNotFound {
                meeting_id: meeting_id.to_string(),
                filename: image.to_string(),
            })
        }
    };
    if size > MAX_FILE_BYTES {
        return Err(ToolError::InvalidArgument {
            message: format!("{image} is {size} bytes — too large to serve"),
        });
    }
    let bytes = std::fs::read(&path).map_err(|e| ToolError::InvalidArgument {
        message: format!("could not read {image}: {e}"),
    })?;

    let Some(ext) = embral_notes::assets::sniff_image_ext(&bytes) else {
        return Err(ToolError::InvalidArgument {
            message: format!("{image} is not an image this server can return"),
        });
    };

    let image_text = db
        .image_text(meeting_id)
        .map_err(ToolError::Db)?
        .into_iter()
        .find(|(name, _)| name == image)
        .map(|(_, text)| text)
        .filter(|text| embral_notes::ocr::is_usable(text));

    // Header-only dimension read; full decode only when the image is over
    // budget.
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| ToolError::InvalidArgument {
            message: format!("{image} did not decode: {e}"),
        })?;
    let (width, height) = reader.into_dimensions().map_err(|e| ToolError::InvalidArgument {
        message: format!("{image} did not decode: {e}"),
    })?;

    if width.max(height) <= MAX_EDGE && bytes.len() <= MAX_BYTES {
        return Ok(FetchedImage {
            mime: mime_for(ext),
            bytes,
            width,
            height,
            scaled: false,
            image_text,
        });
    }

    // Over budget: decode under hard limits (a crafted header must fail
    // loudly, not allocate), resize, re-encode.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| ToolError::InvalidArgument {
            message: format!("{image} did not decode: {e}"),
        })?;
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| ToolError::InvalidArgument {
        message: format!("{image} did not decode: {e}"),
    })?;

    let resized = if decoded.width().max(decoded.height()) > MAX_EDGE {
        decoded.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    let (out_w, out_h) = (resized.width(), resized.height());

    // JPEG input stays JPEG; everything else tries lossless PNG first and
    // falls to JPEG only when the PNG cannot make the budget.
    let (out_bytes, out_mime) = if ext == "jpg" {
        (encode_jpeg(&resized)?, "image/jpeg")
    } else {
        let png = encode_png(&resized)?;
        if png.len() <= MAX_BYTES {
            (png, "image/png")
        } else {
            (encode_jpeg(&resized)?, "image/jpeg")
        }
    };

    Ok(FetchedImage {
        bytes: out_bytes,
        mime: out_mime,
        width: out_w,
        height: out_h,
        scaled: true,
        image_text,
    })
}

fn encode_png(img: &image::DynamicImage) -> Result<Vec<u8>, ToolError> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| ToolError::InvalidArgument {
            message: format!("re-encode failed: {e}"),
        })?;
    Ok(out.into_inner())
}

fn encode_jpeg(img: &image::DynamicImage) -> Result<Vec<u8>, ToolError> {
    let mut out = std::io::Cursor::new(Vec::new());
    // JPEG has no alpha; flatten before encoding.
    let rgb = image::DynamicImage::ImageRgb8(img.to_rgb8());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80);
    encoder
        .encode_image(&rgb)
        .map_err(|e| ToolError::InvalidArgument {
            message: format!("re-encode failed: {e}"),
        })?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_meeting(id: &str) -> Db {
        let db = Db::open_in_memory().unwrap();
        db.upsert_meeting(&embral_db::MeetingRow {
            id: id.to_string(),
            title: "Planning".to_string(),
            started_at: chrono::Utc::now(),
            duration_seconds: 60,
            summary: String::new(),
            transcript: String::new(),
            attendees: Vec::new(),
            audio_path: String::new(),
        })
        .unwrap();
        db
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(
            width,
            height,
            |x, y| image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 253) as u8]),
        ));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn base(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("embral-mcp-img-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Companion to the manual stdio drive (not a test of anything):
    /// builds a persistent scratch library with one meeting and one
    /// oversized image, at the directory EMBRAL_DRIVE_DIR names.
    /// `EMBRAL_DRIVE_DIR=... cargo test -p embral-mcp build_drive_fixture -- --ignored`
    #[test]
    #[ignore = "builds the stdio-drive fixture; needs EMBRAL_DRIVE_DIR"]
    fn build_drive_fixture() {
        let dir = std::path::PathBuf::from(
            std::env::var("EMBRAL_DRIVE_DIR").expect("set EMBRAL_DRIVE_DIR"),
        );
        std::fs::create_dir_all(dir.join("assets/m1")).unwrap();
        let db = Db::open(&dir.join("embral.db")).unwrap();
        db.upsert_meeting(&embral_db::MeetingRow {
            id: "m1".into(),
            title: "Drive fixture".into(),
            started_at: chrono::Utc::now(),
            duration_seconds: 60,
            summary: String::new(),
            transcript: String::new(),
            attendees: Vec::new(),
            audio_path: String::new(),
        })
        .unwrap();
        db.set_image_text("m1", "img-01.png", "the quarterly numbers on one slide", "windows")
            .unwrap();
        std::fs::write(dir.join("assets/m1/img-01.png"), png_bytes(3000, 2000)).unwrap();
    }

    #[test]
    fn the_guard_takes_only_bare_names() {
        for bad in [
            "..",
            ".",
            "",
            "a/b",
            r"a\b",
            "/abs",
            r"C:\x",
            "img-01.png:stream",
            "a\0b",
        ] {
            assert!(require_bare_component(bad, "image").is_err(), "{bad:?}");
        }
        assert!(require_bare_component("img-01.png", "image").is_ok());
    }

    #[test]
    fn a_missing_meeting_answers_before_any_file_is_touched() {
        let dir = base("nomeeting");
        let db = Db::open_in_memory().unwrap();
        match fetch(&dir, &db, "m1", "img-01.png") {
            Err(ToolError::MeetingNotFound { .. }) => {}
            other => panic!("expected MeetingNotFound, got {:?}", other.err().map(|e| e.code())),
        }
    }

    #[test]
    fn an_in_budget_png_passes_through_byte_identical() {
        let dir = base("small");
        std::fs::create_dir_all(dir.join("assets/m1")).unwrap();
        let bytes = png_bytes(200, 120);
        std::fs::write(dir.join("assets/m1/img-01.png"), &bytes).unwrap();
        let db = db_with_meeting("m1");

        let got = fetch(&dir, &db, "m1", "img-01.png").unwrap();
        assert_eq!(got.bytes, bytes);
        assert_eq!(got.mime, "image/png");
        assert_eq!((got.width, got.height), (200, 120));
        assert!(!got.scaled);
        assert!(got.image_text.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_oversized_image_comes_back_at_the_edge_cap() {
        let dir = base("big");
        std::fs::create_dir_all(dir.join("assets/m1")).unwrap();
        std::fs::write(dir.join("assets/m1/img-01.png"), png_bytes(3200, 1600)).unwrap();
        let db = db_with_meeting("m1");

        let got = fetch(&dir, &db, "m1", "img-01.png").unwrap();
        assert!(got.scaled);
        assert_eq!(got.width, MAX_EDGE);
        assert_eq!(got.height, MAX_EDGE / 2);
        assert!(got.bytes.len() <= MAX_BYTES, "{} bytes", got.bytes.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn junk_and_absence_answer_differently() {
        let dir = base("junk");
        std::fs::create_dir_all(dir.join("assets/m1")).unwrap();
        std::fs::write(dir.join("assets/m1/notes.txt"), b"not an image").unwrap();
        let db = db_with_meeting("m1");

        match fetch(&dir, &db, "m1", "notes.txt") {
            Err(e) => assert_eq!(e.code(), "INVALID_ARGUMENT"),
            Ok(_) => panic!("junk served as an image"),
        }
        match fetch(&dir, &db, "m1", "img-09.png") {
            Err(e) => assert_eq!(e.code(), "IMAGE_NOT_FOUND"),
            Ok(_) => panic!("missing file served"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bomb_header_is_refused_not_allocated() {
        // A real PNG header claiming absurd dimensions: the reader's
        // limits refuse it during decode.
        let dir = base("bomb");
        std::fs::create_dir_all(dir.join("assets/m1")).unwrap();
        // Craft: take a valid small png and patch the IHDR dimensions.
        let mut bytes = png_bytes(64, 64);
        // IHDR starts at offset 16: width u32 BE, height u32 BE.
        bytes[16..20].copy_from_slice(&60_000u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&60_000u32.to_be_bytes());
        std::fs::write(dir.join("assets/m1/img-01.png"), &bytes).unwrap();
        let db = db_with_meeting("m1");

        match fetch(&dir, &db, "m1", "img-01.png") {
            Err(e) => assert_eq!(e.code(), "INVALID_ARGUMENT"),
            Ok(_) => panic!("bomb decoded"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stamped_usable_ocr_text_rides_along() {
        let dir = base("ocr");
        std::fs::create_dir_all(dir.join("assets/m1")).unwrap();
        std::fs::write(dir.join("assets/m1/img-01.png"), png_bytes(100, 100)).unwrap();
        std::fs::write(dir.join("assets/m1/img-02.png"), png_bytes(100, 100)).unwrap();
        let db = db_with_meeting("m1");
        db.set_image_text("m1", "img-01.png", "Q3 revenue up 12 percent", "windows")
            .unwrap();
        db.set_image_text("m1", "img-02.png", "|| ~", "windows").unwrap();

        let readable = fetch(&dir, &db, "m1", "img-01.png").unwrap();
        assert_eq!(readable.image_text.as_deref(), Some("Q3 revenue up 12 percent"));
        // A stamped-but-unusable reading stays out of the result.
        let noise = fetch(&dir, &db, "m1", "img-02.png").unwrap();
        assert!(noise.image_text.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_inventory_lists_files_sorted_and_refuses_traversal() {
        let dir = base("list");
        std::fs::create_dir_all(dir.join("assets/m1")).unwrap();
        std::fs::write(dir.join("assets/m1/img-02.png"), b"x").unwrap();
        std::fs::write(dir.join("assets/m1/img-01.png"), b"x").unwrap();
        assert_eq!(list(&dir, "m1"), vec!["img-01.png", "img-02.png"]);
        assert!(list(&dir, "../m1").is_empty());
        assert!(list(&dir, "absent").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

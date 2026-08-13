//! Images pasted into a meeting's documents: where their bytes live, what
//! the markdown points at, and the pure text work around them.
//!
//! **Assets are keyed by meeting id, never by title.** `assets/{meeting_id}/`
//! survives a rename; a title stem would mean moving the directory and
//! rewriting every link in two documents each time the user renames a
//! meeting. The markdown stores the link relative to the storage root, so
//! one resolver serves rendering (relative → absolute → `convertFileSrc`)
//! and export (relative → vault-relative).
//!
//! **They are meeting-scoped and die only with their meeting.** Never
//! collect one because a document stopped referencing it: the summary and
//! the notes share an asset directory, so deleting an image from the notes
//! must not break the copy the summary placed.

use std::collections::HashSet;

/// Where one meeting's images live, relative to the storage root: an
/// indexed path for the app's `resolve_indexed_path` guard.
pub fn asset_dir_rel(meeting_id: &str) -> String {
    format!("assets/{meeting_id}")
}

/// What the markdown holds: the same path, which is also the indexed path.
pub fn link_rel(meeting_id: &str, filename: &str) -> String {
    format!("assets/{meeting_id}/{filename}")
}

/// The image type, read from the bytes themselves. A caller's claimed
/// extension is not evidence: the clipboard is not a trusted source, and
/// the file name is ours to choose anyway.
pub fn sniff_image_ext(bytes: &[u8]) -> Option<&'static str> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.starts_with(PNG) {
        return Some("png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

/// The next free `img-NN.{ext}` given what is already in the directory.
/// Numbering is per directory, not per type, so the order images were
/// pasted in stays readable in a file listing.
pub fn next_asset_name(existing: &[String], ext: &str) -> String {
    let highest = existing
        .iter()
        .filter_map(|name| {
            let stem = name.split('.').next()?;
            stem.strip_prefix("img-")?.parse::<u32>().ok()
        })
        .max()
        .unwrap_or(0);
    format!("img-{:02}.{ext}", highest + 1)
}

/// Every image link target in a markdown document, in order of appearance.
pub fn image_links(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = markdown.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'!' && bytes[i + 1] == b'[' {
            // `![alt](target)`: alt text may not contain `]`, which is
            // markdown's own rule for an unescaped label.
            if let Some(close) = markdown[i + 2..].find(']') {
                let after = i + 2 + close + 1;
                if markdown[after..].starts_with('(') {
                    if let Some(end) = markdown[after + 1..].find(')') {
                        let target = &markdown[after + 1..after + 1 + end];
                        // A title after the URL (`(path "alt")`) is legal.
                        let target = target.split_whitespace().next().unwrap_or(target);
                        out.push(target.to_string());
                        i = after + 1 + end;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

/// Drop image links the meeting does not actually own.
///
/// The summary is written by a language model that has been shown the
/// notes' image links and asked to reuse them. Asking nicely is not a
/// guarantee: it can invent a plausible-looking path, and the notes it was
/// shown are user-typed, so `![](../../../secret.png)` is reachable too.
/// The alt text and surrounding prose stay; only the link goes.
pub fn retain_known_images(markdown: &str, known: &HashSet<String>) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(start) = rest.find("![") {
        let (before, from_bang) = rest.split_at(start);
        out.push_str(before);
        let Some(close) = from_bang[2..].find(']') else {
            out.push_str(from_bang);
            return out;
        };
        let after_label = 2 + close + 1;
        if !from_bang[after_label..].starts_with('(') {
            out.push_str(&from_bang[..after_label]);
            rest = &from_bang[after_label..];
            continue;
        }
        let Some(end) = from_bang[after_label + 1..].find(')') else {
            out.push_str(from_bang);
            return out;
        };
        let whole_end = after_label + 1 + end + 1;
        let target = &from_bang[after_label + 1..after_label + 1 + end];
        let target = target.split_whitespace().next().unwrap_or(target);
        if known.contains(target) {
            out.push_str(&from_bang[..whole_end]);
        }
        rest = &from_bang[whole_end..];
    }
    out.push_str(rest);
    out
}

/// Repoint every `assets/…` link at a new prefix: how a document reaches a
/// vault, where the images sit beside the note rather than under the
/// storage root.
pub fn rewrite_asset_links(markdown: &str, to_prefix: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(start) = rest.find("](assets/") {
        let (before, from_bracket) = rest.split_at(start);
        out.push_str(before);
        out.push_str("](");
        let target_start = 2; // past "]("
        let Some(end) = from_bracket[target_start..].find(')') else {
            out.push_str(&from_bracket[target_start..]);
            return out;
        };
        let target = &from_bracket[target_start..target_start + end];
        // `assets/{id}/{file}` → `{prefix}/{id}/{file}`
        let tail = target.strip_prefix("assets/").unwrap_or(target);
        out.push_str(&format!("{}/{}", to_prefix.trim_end_matches('/'), tail));
        rest = &from_bracket[target_start + end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_keyed_by_meeting_id() {
        assert_eq!(asset_dir_rel("260326T143000_a3f9b2"), "assets/260326T143000_a3f9b2");
        assert_eq!(
            link_rel("260326T143000_a3f9b2", "img-01.png"),
            "assets/260326T143000_a3f9b2/img-01.png"
        );
    }

    #[test]
    fn the_type_comes_from_the_bytes() {
        assert_eq!(sniff_image_ext(&[0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0]), Some("png"));
        assert_eq!(sniff_image_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(sniff_image_ext(b"GIF89a....."), Some("gif"));
        let mut webp = b"RIFF____WEBPVP8 ".to_vec();
        webp.push(0);
        assert_eq!(sniff_image_ext(&webp), Some("webp"));
        // Anything else is refused rather than stored under a guess.
        assert_eq!(sniff_image_ext(b"<html>hi"), None);
        assert_eq!(sniff_image_ext(b""), None);
    }

    #[test]
    fn names_continue_from_the_highest_present() {
        assert_eq!(next_asset_name(&[], "png"), "img-01.png");
        assert_eq!(next_asset_name(&["img-01.png".into()], "jpg"), "img-02.jpg");
        // Numbering is per directory, so a gap does not get reused: two
        // images never share a name even if one was deleted by hand.
        assert_eq!(
            next_asset_name(&["img-01.png".into(), "img-07.webp".into()], "png"),
            "img-08.png"
        );
        // Files that are not ours are ignored rather than confusing it.
        assert_eq!(next_asset_name(&["notes.txt".into()], "png"), "img-01.png");
    }

    #[test]
    fn image_links_are_found_but_plain_links_are_not() {
        let md = "text ![one](assets/m1/img-01.png) and [a link](https://x.test) and \
                  ![two](assets/m1/img-02.png \"a title\")";
        assert_eq!(
            image_links(md),
            vec!["assets/m1/img-01.png", "assets/m1/img-02.png"]
        );
    }

    #[test]
    fn unknown_image_links_are_dropped_and_known_ones_kept() {
        let known: HashSet<String> = ["assets/m1/img-01.png".to_string()].into_iter().collect();
        let md = "keep ![a](assets/m1/img-01.png) drop ![b](assets/m1/img-99.png) end";
        assert_eq!(
            retain_known_images(md, &known),
            "keep ![a](assets/m1/img-01.png) drop  end"
        );
    }

    #[test]
    fn a_traversal_link_never_survives() {
        // Reachable through the user's own notes, which the summary prompt
        // quotes verbatim.
        let known: HashSet<String> = ["assets/m1/img-01.png".to_string()].into_iter().collect();
        let md = "![x](../../../secret.png) ![y](assets/m1/img-01.png)";
        let out = retain_known_images(md, &known);
        assert!(!out.contains("secret.png"), "{out}");
        assert!(out.contains("assets/m1/img-01.png"), "{out}");
    }

    #[test]
    fn prose_without_images_is_returned_unchanged() {
        let known = HashSet::new();
        let md = "# Title\n\nA paragraph with [a link](https://x.test) in it.\n";
        assert_eq!(retain_known_images(md, &known), md);
    }

    #[test]
    fn export_repoints_asset_links_at_the_vault() {
        let md = "![a](assets/m1/img-01.png)\n\n![b](assets/m1/img-02.png)";
        assert_eq!(
            rewrite_asset_links(md, "embral-assets"),
            "![a](embral-assets/m1/img-01.png)\n\n![b](embral-assets/m1/img-02.png)"
        );
        // Links that are not assets are left exactly as they are.
        let other = "[docs](https://embral.app) and ![remote](https://x.test/i.png)";
        assert_eq!(rewrite_asset_links(other, "embral-assets"), other);
    }
}

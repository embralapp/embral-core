//! Post-meeting integrations: mirror notes into an Obsidian vault (or any
//! folder) and deliver the meeting-finished webhook payload. Best-effort
//! side effects the Tauri app fires after a meeting's index entry is
//! written; neither may block or fail the core save (mirroring the
//! existing non-fatal MP3/LLM handling).
//!
//! Wire concerns are kept pure where possible: [`render_filename`],
//! [`compose_export`], [`to_inline_metadata`], and [`webhook_payload`] are
//! unit-tested; the IO/network wrappers ([`export_to_obsidian`],
//! [`send_webhook`]) are thin — retries and failure surfacing belong to
//! the caller.

use anyhow::Result;
use chrono::{DateTime, Utc};
use embral_types::{ExportMetadataFormat, MeetingRecord, WebhookMethod};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::text::sanitize_filename;

/// Render an export filename stem from a user template. Tokens: `{date}`
/// (YYYY-MM-DD), `{time}` (HH-MM), `{year}` `{month}` `{day}` `{hour}`
/// `{minute}`, and `{title}` (slugified: lowercase, words joined by `-`).
/// The result is filesystem-safe and never empty; callers append the
/// extension. Internal library filenames are unaffected by this — it applies
/// to exported copies only.
pub fn render_filename(template: &str, title: &str, started_at: &DateTime<Utc>) -> String {
    let slug: String = {
        let lowered = title.to_lowercase();
        let mut out = String::with_capacity(lowered.len());
        let mut prev_dash = false;
        for c in lowered.chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c);
                prev_dash = false;
            } else if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        }
        let out = out.trim_matches('-').to_string();
        if out.is_empty() {
            "untitled".to_string()
        } else {
            out
        }
    };

    let template = if template.trim().is_empty() {
        "{date}-{time}-{title}"
    } else {
        template
    };
    let rendered = template
        .replace("{date}", &started_at.format("%Y-%m-%d").to_string())
        .replace("{time}", &started_at.format("%H-%M").to_string())
        .replace("{year}", &started_at.format("%Y").to_string())
        .replace("{month}", &started_at.format("%m").to_string())
        .replace("{day}", &started_at.format("%d").to_string())
        .replace("{hour}", &started_at.format("%H").to_string())
        .replace("{minute}", &started_at.format("%M").to_string())
        .replace("{title}", &slug);

    let safe = sanitize_filename(rendered.trim());
    if safe.trim().is_empty() {
        "untitled".to_string()
    } else {
        safe
    }
}

/// Drop a leading `# Heading` line, so a caller can render the title itself.
fn strip_leading_h1(markdown: &str) -> &str {
    let trimmed = markdown.trim_start();
    match trimmed.strip_prefix("# ") {
        Some(rest) => rest.split_once('\n').map(|(_, body)| body).unwrap_or(""),
        None => trimmed,
    }
    .trim_start_matches('\n')
}

/// Drop one leading `---`-fenced frontmatter block. The summary contract
/// makes the model open with its own block ([prompt.rs] OUTPUT_CONTRACT);
/// the export composes the canonical one, and two YAML blocks make every
/// reader render the second as body text. The close must sit at a line
/// boundary; with no valid close this is a document that happens to open
/// with a horizontal rule, and it passes through untouched.
fn strip_leading_frontmatter(markdown: &str) -> &str {
    let trimmed = markdown.trim_start();
    let Some(after_open) = trimmed.strip_prefix("---\n") else {
        return trimmed;
    };
    for (idx, _) in after_open.match_indices("\n---") {
        let after = &after_open[idx + "\n---".len()..];
        if after.is_empty() || after.starts_with('\n') {
            return after.trim_start_matches('\n');
        }
    }
    trimmed
}

/// The document that leaves the app: what the meeting produced, filtered by
/// the user's include switches ([configuration.md]). Each content argument is
/// `None` when its switch is off — no section at all — and `Some` when
/// included; an included-but-empty summary or notes section still disappears
/// rather than exporting a heading with nothing under it, while an included
/// empty transcript says so. All three off yields a metadata stub
/// (frontmatter + title), deliberately.
pub fn compose_export(
    frontmatter: &str,
    title: &str,
    summary_body: Option<&str>,
    user_notes: Option<&str>,
    transcript_text: Option<&str>,
) -> String {
    let mut out = String::new();
    let frontmatter = frontmatter.trim_end();
    if !frontmatter.is_empty() {
        out.push_str(frontmatter);
        out.push('\n');
    }

    let title = if title.trim().is_empty() {
        "Untitled Meeting"
    } else {
        title.trim()
    };
    out.push_str(&format!("# {title}\n"));

    if let Some(body) = summary_body {
        // Fence first: the model's H1 sits below its frontmatter, so the
        // H1 strip only sees it once the fence is gone.
        let summary = strip_leading_h1(strip_leading_frontmatter(body)).trim();
        if !summary.is_empty() {
            out.push_str(&format!("\n{summary}\n"));
        }
    }

    if let Some(notes) = user_notes {
        let notes = notes.trim();
        if !notes.is_empty() {
            out.push_str(&format!("\n## My notes\n\n{notes}\n"));
        }
    }

    if let Some(transcript) = transcript_text {
        let transcript = transcript.trim();
        let transcript = if transcript.is_empty() {
            "_No transcript segments were captured._"
        } else {
            transcript
        };
        out.push_str(&format!("\n## Transcript\n\n{transcript}\n"));
    }

    out
}

/// Convert a notes document's YAML frontmatter into a human-readable block
/// under the H1 (the "Inline" metadata style): the frontmatter is removed and
/// `**Date:** … / **Duration:** … / **Participants:** …` lines are inserted
/// after the title heading. Documents without frontmatter pass through
/// unchanged.
pub fn to_inline_metadata(markdown: &str) -> String {
    let Some((fields, body)) = split_frontmatter(markdown) else {
        return markdown.to_string();
    };

    let mut meta_lines: Vec<String> = Vec::new();
    if let Some(start) = fields
        .get("start_time")
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
    {
        meta_lines.push(format!(
            "**Date:** {}",
            start.format("%A, %B %-d, %Y %H:%M")
        ));
    }
    if let Some(mins) = fields.get("duration_minutes") {
        meta_lines.push(format!("**Duration:** {} minutes", mins));
    }
    if let Some(attendees) = fields.get("attendees") {
        let names: Vec<String> = serde_json::from_str(attendees).unwrap_or_default();
        if !names.is_empty() {
            meta_lines.push(format!("**Participants:** {}", names.join(", ")));
        }
    }
    if meta_lines.is_empty() {
        return body.to_string();
    }
    let block = meta_lines.join("\n\n");

    // Insert after the first H1 when present, else prepend.
    let mut out: Vec<&str> = Vec::new();
    let mut inserted = false;
    for line in body.lines() {
        out.push(line);
        if !inserted && line.starts_with("# ") {
            out.push("");
            out.push(&block);
            inserted = true;
        }
    }
    if !inserted {
        return format!("{}\n\n{}", block, body.trim_start());
    }
    out.join("\n")
}

/// Parse a leading YAML frontmatter block into (fields, body-after-block).
fn split_frontmatter(
    markdown: &str,
) -> Option<(std::collections::HashMap<String, String>, &str)> {
    let rest = markdown.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let block = &rest[..end];
    let after = &rest[end + 4..];
    let body = after
        .strip_prefix('\n')
        .unwrap_or(after)
        .trim_start_matches('\n');

    let mut fields = std::collections::HashMap::new();
    for line in block.lines() {
        if let Some((k, v)) = line.split_once(':') {
            fields.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    Some((fields, body))
}

/// Write the meeting's notes into `vault_dir` (created if missing), named by
/// the user's filename template, with metadata rendered per `format`.
/// Returns the path written.
/// Where images land inside the vault. A single directory beside the notes,
/// sub-foldered by meeting so two meetings' `img-01.png` never collide.
pub const VAULT_ASSET_DIR: &str = "embral-assets";

pub fn export_to_obsidian(
    vault_dir: &str,
    record: &MeetingRecord,
    summary: &str,
    filename_template: &str,
    format: ExportMetadataFormat,
    // `storage_base` is the library root, so an `assets/…` link can be
    // resolved and copied. `None` skips the copying; the links are still
    // repointed, which is the honest outcome — a broken link the user can
    // see beats an image that silently isn't there.
    storage_base: Option<&Path>,
) -> Result<PathBuf> {
    let dir = Path::new(vault_dir);
    std::fs::create_dir_all(dir)?;
    let stem = render_filename(filename_template, &record.title, &record.date);
    let path = dir.join(format!("{stem}.md"));

    // Copy the images first, so the note is never written pointing at files
    // that are not there yet.
    if let Some(base) = storage_base {
        for link in crate::assets::image_links(summary) {
            let Some(tail) = link.strip_prefix("assets/") else {
                continue;
            };
            let from = base.join("assets").join(tail);
            let to = dir.join(VAULT_ASSET_DIR).join(tail);
            if !from.is_file() {
                continue;
            }
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&from, &to)?;
        }
    }

    // One pass over the composed document covers the summary body and the
    // "## My notes" section alike.
    let summary = crate::assets::rewrite_asset_links(summary, VAULT_ASSET_DIR);
    let content = match format {
        // Frontmatter passes through — Obsidian reads it as note properties.
        ExportMetadataFormat::Frontmatter => summary,
        ExportMetadataFormat::Inline => to_inline_metadata(&summary),
    };
    std::fs::write(&path, content)?;
    Ok(path)
}

/// The full-content half of a webhook payload, present only for
/// destinations that opted in ([integrations.md] §Webhooks).
pub struct WebhookContent<'a> {
    /// The meeting summary; empty when summaries are off.
    pub summary_markdown: &'a str,
    /// The user's own notes.
    pub notes_markdown: &'a str,
    pub transcript_markdown: &'a str,
}

/// The JSON body sent to a webhook destination when a meeting finishes.
/// Stable, self-describing shape so downstream automations (Zapier, n8n, a
/// homelab script) can consume it without scraping files. Metadata always;
/// the content fields exist only when the destination opted in — absent
/// rather than empty, so a consumer can tell "not sent" from "empty".
pub fn webhook_payload(record: &MeetingRecord, content: Option<&WebhookContent>) -> Value {
    let mut payload = json!({
        "event": "meeting.finished",
        "meeting": {
            "id": record.id,
            "title": record.title,
            "date": record.date,
            "duration_seconds": record.duration_seconds,
        },
    });
    if let Some(content) = content {
        payload["summary_markdown"] = content.summary_markdown.into();
        payload["notes_markdown"] = content.notes_markdown.into();
        payload["transcript_markdown"] = content.transcript_markdown.into();
    }
    payload
}

/// How long one delivery attempt may take end to end; a hung endpoint must
/// not pin the caller's retry task.
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Send one webhook delivery. Thin on purpose — retries and failure
/// surfacing are the caller's; a non-2xx answer is an error here so the
/// caller can retry it.
pub async fn send_webhook(url: &str, method: WebhookMethod, payload: &Value) -> Result<()> {
    let client = reqwest::Client::builder().timeout(SEND_TIMEOUT).build()?;
    let request = match method {
        WebhookMethod::Post => client.post(url),
        WebhookMethod::Put => client.put(url),
    };
    request.json(payload).send().await?.error_for_status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn record() -> MeetingRecord {
        MeetingRecord {
            id: "260326T143000_a3f9b2".into(),
            title: "Q3: Pipeline Review".into(),
            date: Utc.with_ymd_and_hms(2026, 3, 26, 14, 30, 0).unwrap(),
            duration_seconds: 3480,
            chunks: 1,
            audio_path: "audio/x.mp3".into(),
        }
    }

    #[test]
    fn webhook_metadata_payload_has_stable_shape() {
        let p = webhook_payload(&record(), None);
        assert_eq!(p["event"], "meeting.finished");
        assert_eq!(p["meeting"]["id"], "260326T143000_a3f9b2");
        assert_eq!(p["meeting"]["title"], "Q3: Pipeline Review");
        assert_eq!(p["meeting"]["duration_seconds"], 3480);
        // Metadata only: no content field exists until a destination opts
        // in — absent, not empty.
        assert!(p.get("summary_markdown").is_none());
        assert!(p.get("notes_markdown").is_none());
        assert!(p.get("transcript_markdown").is_none());
    }

    #[test]
    fn webhook_content_rides_only_when_opted_in() {
        let content = WebhookContent {
            summary_markdown: "# S",
            notes_markdown: "my notes",
            transcript_markdown: "T",
        };
        let p = webhook_payload(&record(), Some(&content));
        assert_eq!(p["event"], "meeting.finished");
        assert_eq!(p["summary_markdown"], "# S");
        assert_eq!(p["notes_markdown"], "my notes");
        assert_eq!(p["transcript_markdown"], "T");
    }

    #[test]
    fn filename_template_renders_all_tokens() {
        let at = Utc.with_ymd_and_hms(2026, 5, 3, 10, 30, 0).unwrap();
        assert_eq!(
            render_filename("{date}-{time}-{title}", "Weekly Sync", &at),
            "2026-05-03-10-30-weekly-sync"
        );
        assert_eq!(
            render_filename("{year}/{month} {title}", "A B", &at),
            "202605 a-b" // '/' is filesystem-illegal and stripped
        );
    }

    #[test]
    fn filename_slug_strips_punctuation() {
        let at = Utc.with_ymd_and_hms(2026, 5, 3, 10, 30, 0).unwrap();
        assert_eq!(
            render_filename("{title}", "Q3: Pipeline — Review!", &at),
            "q3-pipeline-review"
        );
    }

    #[test]
    fn filename_never_empty() {
        let at = Utc.with_ymd_and_hms(2026, 5, 3, 10, 30, 0).unwrap();
        // Empty template falls back to the default; an unsluggable title
        // becomes "untitled".
        assert_eq!(render_filename("", "***", &at), "2026-05-03-10-30-untitled");
        assert_eq!(render_filename("{title}", "***", &at), "untitled");
    }

    #[test]
    fn inline_metadata_replaces_frontmatter() {
        let md = "---\nstart_time: 2026-05-03T10:30:00Z\nduration_minutes: 45\nmeeting_id: x\nattendees: [\"Alice\",\"Bob\"]\n---\n\n# Weekly Sync\n\nBody text.";
        let out = to_inline_metadata(md);
        assert!(!out.contains("---"));
        assert!(out.starts_with("# Weekly Sync"));
        assert!(out.contains("**Date:** Sunday, May 3, 2026 10:30"));
        assert!(out.contains("**Duration:** 45 minutes"));
        assert!(out.contains("**Participants:** Alice, Bob"));
        assert!(out.contains("Body text."));
    }

    #[test]
    fn inline_metadata_passthrough_without_frontmatter() {
        assert_eq!(to_inline_metadata("# T\n\nbody"), "# T\n\nbody");
    }

    #[test]
    fn export_writes_templated_file_with_inline_metadata() {
        let dir = std::env::temp_dir().join(format!("embral-export-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let notes =
            "---\nstart_time: 2026-03-26T14:30:00Z\nduration_minutes: 58\nmeeting_id: x\nattendees: [\"Sarah\"]\n---\n\n# Q3: Pipeline Review\n\nbody";
        let path = export_to_obsidian(
            dir.to_str().unwrap(),
            &record(),
            notes,
            "{date} {title}",
            ExportMetadataFormat::Inline,
            None,
        )
        .unwrap();
        assert!(path.ends_with("2026-03-26 q3-pipeline-review.md"));
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("**Participants:** Sarah"));
        assert!(!written.starts_with("---"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A vault copy has to stand on its own: the image travels with the
    /// note and the link points at where it landed, not back into the
    /// library the reader may not have.
    #[test]
    fn export_carries_the_images_and_repoints_their_links() {
        let base = std::env::temp_dir().join(format!("embral-exp-lib-{}", std::process::id()));
        let vault = std::env::temp_dir().join(format!("embral-exp-vault-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&vault);
        let assets = base.join("assets").join("260326T143000_a3f9b2");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(assets.join("img-01.png"), b"pretend png").unwrap();

        let doc = "# Q3: Pipeline Review\n\n\
                   ![the chart](assets/260326T143000_a3f9b2/img-01.png)\n";
        let path = export_to_obsidian(
            vault.to_str().unwrap(),
            &record(),
            doc,
            "{title}",
            ExportMetadataFormat::Frontmatter,
            Some(&base),
        )
        .unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("![the chart](embral-assets/260326T143000_a3f9b2/img-01.png)"),
            "{written}"
        );
        assert!(vault
            .join("embral-assets/260326T143000_a3f9b2/img-01.png")
            .is_file());

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&vault);
    }

    const FRONTMATTER: &str = "---\nstart_time: 2026-03-26T14:30:00Z\n---\n";

    /// What the LLM actually returns: the contract's own frontmatter and
    /// title, which the export must not duplicate.
    const CONTRACT_SUMMARY: &str = "---\nstart_time: 2026-03-26T14:30:00Z\nduration_minutes: 45\nmeeting_id: m1\nattendees: [\"Alice\", \"Bob\"]\n---\n\n# Weekly Sync\n\n## Key Takeaways\n\n- Ship it.\n\n## Next Steps\n\n- **Alice** ships it.";

    #[test]
    fn a_contract_shaped_summary_exports_one_frontmatter_and_one_title() {
        let out = compose_export(
            FRONTMATTER,
            "Weekly Sync",
            Some(CONTRACT_SUMMARY),
            None,
            None,
        );
        // Exactly the canonical block and title; the model's copies gone.
        assert!(out.starts_with("---\nstart_time:"));
        assert_eq!(out.matches("---\n").count(), 2, "one fence pair: {out}");
        assert_eq!(out.matches("# Weekly Sync").count(), 1, "{out}");
        assert_eq!(out.matches("duration_minutes").count(), 0, "{out}");
        // The body survived whole.
        assert!(out.contains("## Key Takeaways"));
        assert!(out.contains("**Alice** ships it."));
    }

    #[test]
    fn inline_metadata_mode_carries_no_fence_at_all() {
        // The canonical metadata rides inline elsewhere; the composer gets
        // an empty frontmatter argument, and the model's block must not
        // slip through as body text.
        let out = compose_export("", "Weekly Sync", Some(CONTRACT_SUMMARY), None, None);
        assert!(!out.contains("---\n"), "{out}");
        assert_eq!(out.matches("# Weekly Sync").count(), 1);
        assert!(out.contains("## Key Takeaways"));
    }

    #[test]
    fn a_summary_opening_with_a_horizontal_rule_is_left_alone() {
        // `---` with no closing fence is content, not metadata.
        let out = compose_export(FRONTMATTER, "T", Some("---\n\njust a rule up top"), None, None);
        assert!(out.contains("just a rule up top"));
        assert_eq!(out.matches("---").count(), 3, "canonical pair + the rule: {out}");
    }

    #[test]
    fn the_frontmatter_strip_handles_the_edges() {
        // Close at end-of-text.
        assert_eq!(strip_leading_frontmatter("---\na: 1\n---"), "");
        // Leading whitespace before the fence.
        assert_eq!(strip_leading_frontmatter("\n\n---\na: 1\n---\nbody"), "body");
        // Four dashes open a rule, not a fence.
        assert_eq!(
            strip_leading_frontmatter("----\nnot frontmatter"),
            "----\nnot frontmatter"
        );
        // A four-dash line does not close a fence; the real close does.
        assert_eq!(
            strip_leading_frontmatter("---\na: 1\n----\nb: 2\n---\nbody"),
            "body"
        );
        // No fence at all.
        assert_eq!(strip_leading_frontmatter("plain"), "plain");
    }

    #[test]
    fn export_carries_summary_notes_and_transcript() {
        let out = compose_export(
            FRONTMATTER,
            "Weekly Sync",
            Some("# Weekly Sync\n\n## Decisions\n\nShip it."),
            Some("John: ship on Friday"),
            Some("Alice: are we ready?"),
        );

        // Frontmatter first (Obsidian reads it as note properties), one title.
        assert!(out.starts_with("---\nstart_time:"));
        assert_eq!(out.matches("# Weekly Sync").count(), 1);
        // The summary's own H1 was dropped, its body kept.
        assert!(out.contains("## Decisions"));
        assert!(out.contains("Ship it."));
        assert!(out.contains("## My notes\n\nJohn: ship on Friday"));
        assert!(out.contains("## Transcript\n\nAlice: are we ready?"));
    }

    #[test]
    fn export_without_a_summary_is_still_worth_having() {
        // Summaries off: the export is the user's notes and the transcript,
        // not an empty file.
        let out = compose_export(
            FRONTMATTER,
            "Weekly Sync",
            Some(""),
            Some("my notes"),
            Some("the words"),
        );
        assert!(out.contains("# Weekly Sync"));
        assert!(out.contains("## My notes\n\nmy notes"));
        assert!(out.contains("## Transcript\n\nthe words"));
    }

    #[test]
    fn export_omits_empty_sections() {
        // No summary and no user notes: no headings with nothing under them.
        let out = compose_export(FRONTMATTER, "T", Some(""), Some("   "), Some("the words"));
        assert!(!out.contains("## My notes"));
        assert!(out.contains("## Transcript"));

        // An empty transcript still says so rather than trailing off.
        let empty = compose_export(FRONTMATTER, "T", Some(""), Some(""), Some(""));
        assert!(empty.contains("_No transcript segments were captured._"));
    }

    #[test]
    fn export_honors_the_include_switches() {
        // Transcript excluded: no section, no placeholder — unlike an
        // included-but-empty transcript.
        let out = compose_export(FRONTMATTER, "T", Some("Summary."), Some("notes"), None);
        assert!(!out.contains("## Transcript"));
        assert!(!out.contains("_No transcript segments were captured._"));
        assert!(out.contains("Summary."));
        assert!(out.contains("## My notes"));

        // Summary and notes excluded: the export is just the transcript.
        let out = compose_export(FRONTMATTER, "T", None, None, Some("the words"));
        assert!(out.contains("## Transcript\n\nthe words"));
        assert!(!out.contains("## My notes"));

        // All three off: a metadata stub, deliberately.
        let stub = compose_export(FRONTMATTER, "Weekly Sync", None, None, None);
        assert!(stub.starts_with("---\nstart_time:"));
        assert!(stub.trim_end().ends_with("# Weekly Sync"));
    }

    #[test]
    fn exported_summary_survives_the_inline_metadata_pass() {
        // The composed document is what export_to_obsidian renders, so it has
        // to round-trip through the Inline style too.
        let composed = compose_export(
            "---\nstart_time: 2026-05-03T10:30:00Z\nduration_minutes: 45\nattendees: [\"Alice\"]\n---\n",
            "Weekly Sync",
            Some("Body."),
            Some(""),
            Some("words"),
        );
        let inline = to_inline_metadata(&composed);
        assert!(!inline.starts_with("---"));
        assert!(inline.contains("**Participants:** Alice"));
        assert!(inline.contains("## Transcript"));
    }
}

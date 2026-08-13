//! The pure text work around OCR: tidying what an engine hands back,
//! deciding whether it is worth indexing at all, and squeezing it down for
//! the summary prompt's image inventory.
//!
//! The OS call itself is thin platform code (`platform/{windows,macos}/ocr.rs`);
//! everything that has an opinion about the text lives here, where it can
//! be tested without an OS.

/// Tidy raw engine output into a document.
///
/// Both engines answer in lines (`OcrLine` on Windows, one observation per
/// line on macOS) with whatever spacing their layout analysis produced.
/// Collapse the runs, drop the blanks, and keep the line structure: on a
/// slide the line breaks are the only structure there is.
pub fn normalize(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

// --- Reading order from line geometry ---------------------------------------

/// One recognized line with its box. Any consistent coordinate space works
/// (Windows hands back pixels, Vision normalized units) as long as the
/// origin is top-left; the macOS engine flips Vision's bottom-left y
/// before it gets here.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// A line at least this wide, as a fraction of the global x-extent, is a
/// band separator: it reads alone and cuts the image horizontally, so a
/// full-width title never glues the two columns below it into one cluster.
const BAND_SPAN: f32 = 0.7;

/// Two lines whose x-intervals overlap by at least this fraction of the
/// narrower interval stack into the same column. Measured against the
/// narrower so a ragged short bullet stays with its column.
const COLUMN_OVERLAP: f32 = 0.3;

/// Put lines into reading order using their geometry.
///
/// Both engines emit lines top to bottom across the whole image, which
/// interleaves side-by-side columns into text that reads straight across.
/// This walks the lines in y order, splitting the image into horizontal
/// bands at every full-width line, clusters each band into columns by
/// x-overlap, and reads columns left to right, each top to bottom. A
/// one-column image (every line wide, or everything overlapping) comes
/// out in plain (y, x) order, exactly what the geometry-free path
/// produced. Degenerate boxes never panic and never drop text.
pub fn layout(lines: &[OcrLine]) -> Vec<String> {
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|&a, &b| cmp_yx(&lines[a], &lines[b]));

    let x0 = lines.iter().map(|l| l.x).fold(f32::INFINITY, f32::min);
    let x1 = lines
        .iter()
        .map(|l| l.x + l.width.max(0.0))
        .fold(f32::NEG_INFINITY, f32::max);
    let extent = x1 - x0;
    if !(extent > 0.0) {
        // Empty, a single point, or NaN soup: top-to-bottom is the only
        // order there is.
        return order.iter().map(|&i| lines[i].text.clone()).collect();
    }

    let mut out = Vec::with_capacity(lines.len());
    let mut band: Vec<usize> = Vec::new();
    for &i in &order {
        if lines[i].width >= BAND_SPAN * extent {
            flush_band(lines, &mut band, &mut out);
            out.push(lines[i].text.clone());
        } else {
            band.push(i);
        }
    }
    flush_band(lines, &mut band, &mut out);
    out
}

fn cmp_yx(a: &OcrLine, b: &OcrLine) -> std::cmp::Ordering {
    a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x))
}

/// Whether two lines belong to the same column: their x-intervals overlap
/// by at least `COLUMN_OVERLAP` of the narrower one.
fn same_column(a: &OcrLine, b: &OcrLine) -> bool {
    let (a0, a1) = (a.x, a.x + a.width.max(0.0));
    let (b0, b1) = (b.x, b.x + b.width.max(0.0));
    let overlap = a1.min(b1) - a0.max(b0);
    // Multiply rather than divide: a zero-width interval then never
    // overlaps instead of dividing by zero.
    overlap > 0.0 && overlap >= COLUMN_OVERLAP * a.width.max(0.0).min(b.width.max(0.0))
}

/// Cluster one band into columns (connected components over x-overlap) and
/// emit them left to right, each top to bottom. `band` arrives in (y, x)
/// order and leaves empty.
fn flush_band(lines: &[OcrLine], band: &mut Vec<usize>, out: &mut Vec<String>) {
    let mut columns: Vec<Vec<usize>> = Vec::new();
    for &i in band.iter() {
        // Every column this line touches merges into one: overlap is a
        // pairwise relation and columns are its connected components.
        let hits: Vec<usize> = columns
            .iter()
            .enumerate()
            .filter(|(_, col)| col.iter().any(|&j| same_column(&lines[i], &lines[j])))
            .map(|(c, _)| c)
            .collect();
        match hits.split_first() {
            None => columns.push(vec![i]),
            Some((&first, rest)) => {
                for &c in rest.iter().rev() {
                    let merged = columns.remove(c);
                    columns[first].extend(merged);
                }
                columns[first].push(i);
            }
        }
    }
    columns.sort_by(|a, b| {
        let left = |col: &[usize]| col.iter().map(|&j| lines[j].x).fold(f32::INFINITY, f32::min);
        left(a).total_cmp(&left(b))
    });
    for column in &mut columns {
        column.sort_by(|&a, &b| cmp_yx(&lines[a], &lines[b]));
        out.extend(column.iter().map(|&j| lines[j].text.clone()));
    }
    band.clear();
}

/// A "word" for the purposes below: two or more letters or digits. `Q3`
/// counts; `I`, `||` and `~` do not.
fn word_count(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| w.chars().filter(|c| c.is_alphanumeric()).count() >= 2)
        .count()
}

/// The fewest words worth a chunk of its own. Below this an image is a logo
/// or a decoration, and indexing it costs more in noise than it returns.
const MIN_WORDS: usize = 3;

/// Whether this text is worth indexing.
///
/// An engine pointed at a photo of a wall does not return nothing: it
/// returns a handful of punctuation glyphs it mistook for letters. Those
/// become a chunk, an embedding, and eventually a palette snippet that reads
/// like a bug. Two cheap signals separate a slide from a wall: enough real
/// words, and a majority of the characters actually being letters or digits.
pub fn is_usable(text: &str) -> bool {
    if word_count(text) < MIN_WORDS {
        return false;
    }
    let non_space = text.chars().filter(|c| !c.is_whitespace()).count();
    let alphanumeric = text.chars().filter(|c| c.is_alphanumeric()).count();
    non_space > 0 && alphanumeric * 2 >= non_space
}

/// Split one image's text into passage-sized blocks.
///
/// Most images are one block and stay one chunk. A full-page screenshot is
/// not: left whole it becomes a single oversized chunk whose tail falls off
/// the far side of the embedder's 512-token window, searchable by keyword
/// and invisible to meaning. Splitting on line boundaries keeps every part
/// reachable.
pub fn blocks(text: &str, max_words: usize) -> Vec<String> {
    let max_words = max_words.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut count = 0usize;

    for line in text.lines() {
        let words = line.split_whitespace().count();
        if !current.is_empty() && count + words > max_words {
            out.push(current.join("\n"));
            current.clear();
            count = 0;
        }
        current.push(line);
        count += words;
    }
    if !current.is_empty() {
        out.push(current.join("\n"));
    }
    out
}

/// One line describing an image, for the summary prompt's inventory. The
/// model needs enough to tell one screenshot from another, not the whole
/// slide; the prompt already carries the notes those images sit in.
pub fn for_prompt(text: &str, max_chars: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max_chars {
        return one_line;
    }
    let mut out: String = one_line.chars().take(max_chars).collect();
    // Prefer a word boundary, but not one so early it loses the point.
    if let Some(space) = out.rfind(' ') {
        if space > max_chars / 2 {
            out.truncate(space);
        }
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_spacing_and_keeps_lines() {
        let lines = ["  Q3   revenue ", "", "   ", "4.2M\tvs 3.8M "];
        assert_eq!(normalize(&lines), "Q3 revenue\n4.2M vs 3.8M");
        assert_eq!(normalize(&[]), "");
        assert_eq!(normalize(&["   ", ""]), "");
    }

    #[test]
    fn a_slide_is_usable() {
        assert!(is_usable("Q3 revenue 4.2M\nQ4 forecast 5.1M"));
        assert!(is_usable("Roadmap\nShip the editor\nThen the export"));
    }

    #[test]
    fn a_photo_of_a_wall_is_not() {
        assert!(!is_usable(""));
        assert!(!is_usable("   \n  "));
        // What an engine actually returns from texture and shadow.
        assert!(!is_usable("|| ~ ^^ '"));
        // Enough tokens, but almost none of them are characters.
        assert!(!is_usable("a.b ..... ///// ab cd ef"));
    }

    #[test]
    fn a_word_or_two_is_not_worth_a_chunk() {
        assert!(!is_usable("Roadmap"));
        assert!(!is_usable("ok"));
        assert!(!is_usable("Q3 revenue"));
    }

    #[test]
    fn a_short_image_stays_one_block() {
        let text = "Q3 revenue 4.2M\nQ4 forecast 5.1M";
        assert_eq!(blocks(text, 400), vec![text.to_string()]);
        assert!(blocks("", 400).is_empty());
    }

    #[test]
    fn a_long_image_splits_on_line_boundaries() {
        let line = "one two three four five";
        let text = std::iter::repeat(line).take(10).collect::<Vec<_>>().join("\n");
        // 5 words a line, 10 lines, a 20-word budget → 4 lines per block.
        let out = blocks(&text, 20);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].lines().count(), 4);
        assert_eq!(out[2].lines().count(), 2);
        // Nothing is lost or duplicated in the split.
        assert_eq!(out.join("\n"), text);
    }

    #[test]
    fn one_line_that_alone_exceeds_the_budget_stays_whole() {
        let long = "a b c d e f g h i j k l m n o p";
        assert_eq!(blocks(long, 4), vec![long.to_string()]);
    }

    #[test]
    fn the_prompt_line_is_flattened_and_cut_at_a_word() {
        assert_eq!(for_prompt("Q3\nrevenue  4.2M", 80), "Q3 revenue 4.2M");
        let long = "the quarterly revenue review for the third quarter of the year";
        let cut = for_prompt(long, 20);
        assert!(cut.ends_with('…'), "{cut}");
        assert!(cut.chars().count() <= 21, "{cut}");
        // Cut between words, not through one.
        assert!(long.starts_with(cut.trim_end_matches('…')), "{cut}");
    }

    // --- layout ---------------------------------------------------------

    fn l(text: &str, x: f32, y: f32, w: f32) -> OcrLine {
        OcrLine { text: text.to_string(), x, y, width: w, height: 12.0 }
    }

    /// Every text goes in exactly once and comes out exactly once,
    /// whatever the geometry did to the order.
    fn assert_same_texts(input: &[OcrLine], output: &[String]) {
        let mut want: Vec<&str> = input.iter().map(|l| l.text.as_str()).collect();
        let mut got: Vec<&str> = output.iter().map(String::as_str).collect();
        want.sort_unstable();
        got.sort_unstable();
        assert_eq!(want, got);
    }

    #[test]
    fn layout_of_nothing_is_nothing() {
        assert!(layout(&[]).is_empty());
        assert_eq!(layout(&[l("only", 10.0, 10.0, 100.0)]), vec!["only"]);
    }

    #[test]
    fn a_one_column_document_is_unchanged() {
        // Full-width paragraph lines with a short last line: the everyday
        // shape, and the one whose output must match the geometry-free
        // path exactly.
        let lines = [
            l("first", 40.0, 20.0, 700.0),
            l("second", 40.0, 40.0, 700.0),
            l("short last", 40.0, 60.0, 150.0),
            l("next paragraph", 40.0, 90.0, 700.0),
        ];
        assert_eq!(layout(&lines), vec!["first", "second", "short last", "next paragraph"]);
    }

    #[test]
    fn two_columns_read_left_then_right() {
        let lines = [
            l("L1", 40.0, 100.0, 280.0),
            l("R1", 440.0, 100.0, 280.0),
            l("L2", 40.0, 140.0, 280.0),
            l("R2", 440.0, 140.0, 280.0),
            l("L3", 40.0, 180.0, 280.0),
            l("R3", 440.0, 180.0, 280.0),
        ];
        assert_eq!(layout(&lines), vec!["L1", "L2", "L3", "R1", "R2", "R3"]);
        assert_same_texts(&lines, &layout(&lines));
    }

    #[test]
    fn a_full_width_title_does_not_glue_the_columns() {
        let lines = [
            l("Title", 40.0, 20.0, 680.0),
            l("L1", 40.0, 100.0, 280.0),
            l("R1", 440.0, 100.0, 280.0),
            l("L2", 40.0, 140.0, 280.0),
            l("R2", 440.0, 140.0, 280.0),
        ];
        assert_eq!(layout(&lines), vec!["Title", "L1", "L2", "R1", "R2"]);
    }

    #[test]
    fn bands_order_independently() {
        // A separator between two column pairs: each side of it settles on
        // its own, and the separator reads in its y position.
        let lines = [
            l("Title", 40.0, 20.0, 680.0),
            l("A left", 40.0, 100.0, 280.0),
            l("A right", 440.0, 100.0, 280.0),
            l("Subtitle", 40.0, 200.0, 680.0),
            l("B left", 40.0, 300.0, 280.0),
            l("B right", 440.0, 300.0, 280.0),
        ];
        assert_eq!(
            layout(&lines),
            vec!["Title", "A left", "A right", "Subtitle", "B left", "B right"]
        );
    }

    #[test]
    fn a_ragged_bullet_stays_with_its_column() {
        // The bullet is far narrower than its column, but the overlap is
        // measured against the narrower interval, so it stays put.
        let lines = [
            l("L1", 40.0, 100.0, 280.0),
            l("R1", 440.0, 100.0, 280.0),
            l("- ok", 40.0, 140.0, 60.0),
            l("R2", 440.0, 140.0, 280.0),
        ];
        assert_eq!(layout(&lines), vec!["L1", "- ok", "R1", "R2"]);
    }

    #[test]
    fn a_sliver_of_overlap_does_not_merge_columns() {
        // 20px of overlap against 280px-wide intervals is under the 30%
        // threshold: still two columns.
        let lines = [l("A", 40.0, 100.0, 280.0), l("B", 300.0, 140.0, 280.0)];
        assert_eq!(layout(&lines), vec!["A", "B"]);
    }

    #[test]
    fn transitive_overlap_chains_one_column() {
        // A overlaps B and B overlaps C, but A never touches C; connected
        // components make them one column anyway, in y order.
        let lines = [
            l("A", 0.0, 10.0, 100.0),
            l("D", 400.0, 20.0, 100.0),
            l("B", 60.0, 30.0, 100.0),
            l("C", 120.0, 50.0, 100.0),
        ];
        assert_eq!(layout(&lines), vec!["A", "B", "C", "D"]);
    }

    #[test]
    fn degenerate_boxes_never_drop_text() {
        let lines = [
            l("normal", 40.0, 10.0, 280.0),
            l("zero width", 40.0, 20.0, 0.0),
            l("nan", f32::NAN, 30.0, f32::NAN),
            l("negative width", 40.0, 40.0, -5.0),
        ];
        let out = layout(&lines);
        assert_same_texts(&lines, &out);
    }
}

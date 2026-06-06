//! Shared canonical-form emitters for inline + block markdown.
//!
//! Channels (DOCX, generic-MD, PDF) reconstruct their bodies into the
//! C-7 canonical markdown form. Where that reconstruction is identical
//! across channels it lives here, so the formats emit byte-identical raw
//! `bgraph.md` for equivalent content (the cross-channel emphasis/layout
//! parity the normalization promise calls out as a bonus equivalence).
//!
//! Today: inline emphasis wrapping. Table-layout canonicalization is the
//! next resident (CR-80 #2).

/// Wrap `text` in the canonical emphasis delimiters for the given
/// bold/italic state, keeping any leading/trailing whitespace OUTSIDE
/// the markers.
///
/// CommonMark forbids a closing delimiter preceded by whitespace (so
/// `**bold, **` would not render), and whitespace-outside also stops an
/// adjacent run's markers from fusing (`**bold,** *italic,*` rather than
/// `**bold, ***italic, *`). Whitespace-only / empty runs are returned
/// unchanged — never `** **`.
///
/// Both the DOCX channel (per-run bold/italic flags) and the generic-MD
/// channel (Emphasis/Strong nesting state) emit through this one helper,
/// so equivalent content produces identical emphasis markup.
pub(crate) fn wrap_emphasis(text: &str, bold: bool, italic: bool) -> String {
    if text.is_empty() || text.trim().is_empty() {
        return text.to_string();
    }
    let marks = match (bold, italic) {
        (true, true) => "***",
        (true, false) => "**",
        (false, true) => "*",
        (false, false) => return text.to_string(),
    };
    // Whitespace is ASCII (single-byte) at these boundaries, so the slices
    // are UTF-8-safe.
    let lead = &text[..text.len() - text.trim_start().len()];
    let trail = &text[text.trim_end().len()..];
    let core = text.trim();
    format!("{lead}{marks}{core}{marks}{trail}")
}

/// Per-column horizontal alignment carried by a GFM delimiter row.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Align {
    None,
    Left,
    Right,
    Center,
}

/// Re-format a pipe-delimited table body into the canonical layout: outer
/// pipes, a delimiter (alignment) row, and columns padded to a uniform
/// width. Applied at channel exit to `Table` node bodies so the bare grids
/// the DOCX/PDF channels synthesize and the pretty form the MD channel
/// passes through all converge to one shape.
///
/// - **Idempotent.** Canonical input returns byte-identical.
/// - **Defensive.** Input with no `|` (so not a pipe table) is returned
///   unchanged — keeps non-table bodies (and a parked-PDF channel that
///   never emits pipe tables) safe.
/// - **Layout only.** Content is left-aligned within each column; column
///   alignment (preserved from a source delimiter row, else `None`) lives
///   solely in the delimiter row's colons (prettier-style). This maximizes
///   cross-channel convergence: equivalent tables get identical *content*
///   rows and differ only where the source genuinely declared alignment.
///   Cell text is otherwise preserved verbatim — a re-format, not a re-parse.
///
/// In-cell newlines are unsupported (a cell spanning lines is misread as a
/// row boundary); the channels don't emit them for the corpus.
pub(crate) fn format_pipe_table(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    // No pipe anywhere ⇒ not a pipe table; leave it untouched.
    if lines.is_empty() || !lines.iter().any(|l| l.contains('|')) {
        return text.to_string();
    }

    let mut rows: Vec<Vec<String>> = lines.iter().map(|l| split_row(l)).collect();

    // A delimiter row (all-dashes-with-optional-colons) at position 1 sets
    // column alignment; remove it from the data rows (it's re-synthesized).
    let aligns = if rows.len() >= 2 {
        parse_delimiter_row(&rows[1])
    } else {
        None
    };
    if aligns.is_some() {
        rows.remove(1);
    }

    let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if ncols == 0 {
        return text.to_string();
    }
    for r in &mut rows {
        r.resize(ncols, String::new());
    }
    let aligns: Vec<Align> = match aligns {
        Some(mut a) => {
            a.resize(ncols, Align::None);
            a
        }
        None => vec![Align::None; ncols],
    };

    // Column width = widest cell (min 3, so a delimiter has room for colons).
    let mut widths = vec![3usize; ncols];
    for r in &rows {
        for (j, cell) in r.iter().enumerate() {
            widths[j] = widths[j].max(cell.chars().count());
        }
    }

    let mut out = String::new();
    push_row(&mut out, &rows[0], &widths);
    push_delimiter(&mut out, &aligns, &widths);
    for r in &rows[1..] {
        push_row(&mut out, r, &widths);
    }
    out.pop(); // drop the trailing newline push_row added
    out
}

/// Split a table line into trimmed cells, dropping one optional leading and
/// trailing border pipe and honoring `\|` escapes inside cells.
fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);

    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                cur.push('\\');
                if let Some(&n) = chars.peek() {
                    cur.push(n);
                    chars.next();
                }
            }
            '|' => cells.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    cells.push(cur);
    cells.iter().map(|c| c.trim().to_string()).collect()
}

/// Parse a candidate delimiter row. Returns per-column alignment iff every
/// cell is a valid GFM delimiter (optional leading/trailing `:`, ≥1 `-`).
fn parse_delimiter_row(cells: &[String]) -> Option<Vec<Align>> {
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for cell in cells {
        let c = cell.trim();
        let left = c.starts_with(':');
        let right = c.ends_with(':');
        let start = left as usize;
        let end = c.len().checked_sub(right as usize)?;
        if start > end {
            return None;
        }
        let mid = &c[start..end];
        if mid.is_empty() || !mid.bytes().all(|b| b == b'-') {
            return None;
        }
        aligns.push(match (left, right) {
            (true, true) => Align::Center,
            (true, false) => Align::Left,
            (false, true) => Align::Right,
            (false, false) => Align::None,
        });
    }
    Some(aligns)
}

fn push_row(out: &mut String, cells: &[String], widths: &[usize]) {
    out.push('|');
    for (j, &w) in widths.iter().enumerate() {
        let cell = cells.get(j).map(String::as_str).unwrap_or("");
        let pad = w.saturating_sub(cell.chars().count());
        out.push(' ');
        out.push_str(cell);
        out.extend(std::iter::repeat(' ').take(pad));
        out.push_str(" |");
    }
    out.push('\n');
}

fn push_delimiter(out: &mut String, aligns: &[Align], widths: &[usize]) {
    out.push('|');
    for (j, &w) in widths.iter().enumerate() {
        // Delimiter field spans the content field (width + the 2 pad spaces).
        let field = w + 2;
        match aligns[j] {
            Align::None => out.push_str(&"-".repeat(field)),
            Align::Left => {
                out.push(':');
                out.push_str(&"-".repeat(field - 1));
            }
            Align::Right => {
                out.push_str(&"-".repeat(field - 1));
                out.push(':');
            }
            Align::Center => {
                out.push(':');
                out.push_str(&"-".repeat(field - 2));
                out.push(':');
            }
        }
        out.push('|');
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_emphasis_forms() {
        assert_eq!(wrap_emphasis("x", false, false), "x");
        assert_eq!(wrap_emphasis("x", true, false), "**x**");
        assert_eq!(wrap_emphasis("x", false, true), "*x*");
        assert_eq!(wrap_emphasis("x", true, true), "***x***");
        // Whitespace-only runs are never wrapped (no `** **`).
        assert_eq!(wrap_emphasis(" ", true, false), " ");
        assert_eq!(wrap_emphasis("", true, true), "");
        // Leading/trailing whitespace stays OUTSIDE the markers (valid
        // CommonMark — a closing delimiter may not be preceded by whitespace).
        assert_eq!(wrap_emphasis("bold, ", true, false), "**bold,** ");
        assert_eq!(wrap_emphasis(" italic", false, true), " *italic*");
        assert_eq!(wrap_emphasis("bi, ", true, true), "***bi,*** ");
        // Adjacent differently-emphasised runs must not fuse their delimiters.
        let joined = wrap_emphasis("bold, ", true, false) + &wrap_emphasis("italic, ", false, true);
        assert_eq!(joined, "**bold,** *italic,* ");
    }

    #[test]
    fn pipe_table_docx_bare_grid_gets_pipes_and_delimiter() {
        // DOCX synthesizes a bare grid (no outer pipes, no delimiter row,
        // no padding). Canonical form adds all three; absent alignment ⇒
        // default `None` delimiter.
        let bare = "Name | Value\nAlpha | 42\nBeta | 7";
        assert_eq!(
            format_pipe_table(bare),
            "\
| Name  | Value |
|-------|-------|
| Alpha | 42    |
| Beta  | 7     |",
        );
    }

    #[test]
    fn pipe_table_preserves_source_alignment() {
        // A pretty form with a mixed delimiter row keeps its per-column
        // alignment, but content is left-aligned regardless (prettier-style).
        let pretty = "\
| Left | Mid | Right |
|:-----|:---:|------:|
| a | b | c |";
        assert_eq!(
            format_pipe_table(pretty),
            "\
| Left | Mid | Right |
|:-----|:---:|------:|
| a    | b   | c     |",
        );
    }

    #[test]
    fn pipe_table_is_idempotent() {
        let bare = "Name | Value\nAlpha | 42";
        let once = format_pipe_table(bare);
        assert_eq!(format_pipe_table(&once), once, "second pass must be a no-op");
    }

    #[test]
    fn pipe_table_short_rows_pad_to_column_count() {
        // DOCX emits trailing-empty cells; a short row pads out to the header.
        let bare = "A | B | C\nx | | ";
        assert_eq!(
            format_pipe_table(bare),
            "\
| A   | B   | C   |
|-----|-----|-----|
| x   |     |     |",
        );
    }

    #[test]
    fn pipe_table_non_table_text_passes_through() {
        let prose = "Just a paragraph, no pipes here.";
        assert_eq!(format_pipe_table(prose), prose);
    }
}

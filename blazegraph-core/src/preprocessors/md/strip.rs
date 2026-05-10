//! bgraph fence stripping — bgraph.md → plain markdown / partial bgraph.md.
//!
//! Removes bgraph fence regions from a bgraph.md string per a
//! [`StripMode`]. The three modes mirror the three sed patterns called
//! out in the v1.0.0 wire format spec's "Strip ergonomics" table; the
//! dash discriminator (`bgraph-[a-z-]*` vs. `bgraph[a-z-]*`) is
//! load-bearing.
//!
//! Wire-format definition:
//! `docs/P2/core/architecture/08-bgraph-md-format.md` (v1.0.0).
//!
//! ## Why a line scanner (not regex, not pulldown-cmark)
//!
//! The sed patterns in the spec are correct *as patterns* but a real
//! line-by-line scan handles cases sed cannot:
//!
//! - Non-bgraph code blocks in Section/Paragraph body text (e.g., a
//!   `` ```rust `` block inside a Paragraph) need to pass through
//!   unmodified. A naive regex on multi-line content would risk
//!   gobbling these.
//! - The reserved-prefix invariant (any line starting with
//!   `` ```bgraph `` is reserved by v1.0.0) means we can recognize
//!   fence boundaries by line-prefix alone — no CommonMark parsing
//!   needed.
//!
//! The implementation reuses [`super::bgraph_md::bgraph_fence_open_tag`]
//! and [`super::bgraph_md::is_bare_fence_close`] (both `pub(super)`) so
//! the strip operation and the round-trip parser share the same
//! recognition rules for free.

use super::bgraph_md::{bgraph_fence_open_tag, is_bare_fence_close, strip_trailing_newline};
use super::types::{ParseError, StripMode};

/// Strip bgraph fences from `input` according to `mode`.
///
/// Returns the stripped string. See [`StripMode`] for the three
/// variants.
///
/// Errors:
/// - [`ParseError::MalformedFence`] if the input contains an
///   unterminated bgraph fence (a `` ```bgraph* `` line at column zero
///   without a matching `` ``` `` close before EOF).
///
/// Notes:
/// - Lines outside any bgraph fence are passed through verbatim,
///   including their trailing newlines (so a non-bgraph code block in
///   a Section/Paragraph body, like a `` ```rust `` example, survives
///   intact).
/// - Blank lines that immediately follow a stripped fence are
///   compacted to at most one consecutive blank line in the output —
///   otherwise stripping fences would leave double-blank runs that
///   read like deliberate paragraph breaks. Conservative: a blank line
///   already in the body before stripping is preserved.
pub fn strip(input: &str, mode: StripMode) -> Result<String, ParseError> {
    let mut out = String::with_capacity(input.len());
    let mut iter = input.split_inclusive('\n');
    // Track whether the last emitted line was blank, so we can
    // suppress an additional blank line right after a stripped fence.
    let mut last_emitted_blank = false;
    // Track whether the last action was "stripped a fence" — only
    // suppress one blank after stripping (the natural separator the
    // emitter places between elements); body-internal double blanks
    // pass through unchanged.
    let mut just_stripped = false;

    while let Some(line) = iter.next() {
        let trimmed = strip_trailing_newline(line);
        if let Some(tag) = bgraph_fence_open_tag(trimmed) {
            if should_strip(&tag, mode) {
                // Consume body lines through the fence close. We
                // accept that the inner body is whatever's between
                // the fence-open and fence-close lines; for noise-only
                // mode the Header/Footer/Margin running text is
                // intentionally removed along with the metadata line.
                let mut closed = false;
                for body_line in iter.by_ref() {
                    let body_trimmed = strip_trailing_newline(body_line);
                    if is_bare_fence_close(body_trimmed) {
                        closed = true;
                        break;
                    }
                    // Other ``` content lives inside the fence; we
                    // drop it. If a `` ```bgraph* `` line appears
                    // inside an open fence, that's the same reserved-
                    // prefix violation the round-trip parser flags —
                    // but strip is meant to be tolerant: we drop it
                    // along with the rest. The round-trip parser is
                    // the strict gatekeeper; strip is best-effort.
                    let _ = body_trimmed;
                }
                if !closed {
                    return Err(ParseError::MalformedFence(format!(
                        "unterminated bgraph fence with tag {tag:?} during strip"
                    )));
                }
                just_stripped = true;
                continue;
            } else {
                // Fence not stripped — emit it verbatim, including the
                // body and close. Walk until close, copying every line
                // through.
                out.push_str(line);
                last_emitted_blank = false;
                just_stripped = false;
                let mut closed = false;
                for body_line in iter.by_ref() {
                    out.push_str(body_line);
                    let body_trimmed = strip_trailing_newline(body_line);
                    last_emitted_blank = body_trimmed.is_empty();
                    if is_bare_fence_close(body_trimmed) {
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err(ParseError::MalformedFence(format!(
                        "unterminated bgraph fence with tag {tag:?} during strip"
                    )));
                }
                continue;
            }
        }

        // Non-fence line: emit. Suppress a single blank line that
        // immediately follows a stripped fence — the emitter places a
        // single blank between elements as a separator, and removing
        // the fence leaves that blank as orphaned whitespace.
        let is_blank = trimmed.trim().is_empty();
        if just_stripped && is_blank {
            // Drop this orphan blank; do not flip last_emitted_blank.
            just_stripped = false;
            continue;
        }
        // Otherwise emit. Coalesce consecutive blank lines down to one
        // only across a strip boundary — see test cases for the exact
        // shape.
        if is_blank && last_emitted_blank && just_stripped {
            just_stripped = false;
            continue;
        }
        out.push_str(line);
        last_emitted_blank = is_blank;
        just_stripped = false;
    }

    Ok(out)
}

/// Whether a fence with the given `tag` should be stripped under `mode`.
///
/// Tags here are the un-decorated names from
/// [`super::bgraph_md::bgraph_fence_open_tag`]: `bgraph`,
/// `bgraph-bookmarks`, `bgraph-section`, `bgraph-paragraph`,
/// `bgraph-header`, `bgraph-footer`, `bgraph-margin`. Any other
/// `bgraph*` tag is a reserved-prefix violation; the round-trip
/// parser flags it but `strip` is tolerant — we treat the tag as
/// matching the broader `bgraph*` pattern (it will be stripped in
/// BodyOnly mode and ignored otherwise).
fn should_strip(tag: &str, mode: StripMode) -> bool {
    match mode {
        // `bgraph[a-z-]*` — every bgraph fence (with or without dash).
        StripMode::BodyOnly => true,
        // `bgraph-[a-z-]*` — every dashed fence; doc-level `bgraph`
        // (no dash) survives.
        StripMode::KeepMetadata => tag != "bgraph",
        // `bgraph-(header|footer|margin)` — only the three noise tags.
        StripMode::NoiseOnly => matches!(tag, "bgraph-header" | "bgraph-footer" | "bgraph-margin"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but realistic emitter-style bgraph.md sample. The
    /// blank-line separators between elements mirror what
    /// [`crate::graphs::serialization::markdown::emit_markdown`]
    /// produces in practice.
    fn sample_bgraph_md() -> String {
        [
            "```bgraph",
            "{\"schema\":\"1.0.0\",\"blazegraph_version\":\"0.6.0\",\"source\":{\"format\":\"pdf\",\"filename\":\"x.pdf\",\"sha256\":\"src-sha\"},\"flow_type\":\"Fixed\",\"title\":\"Sample\",\"config_hash\":\"cfg-sha\",\"graph_sha256\":\"deadbeef\"}",
            "```",
            "",
            "```bgraph-bookmarks",
            "{\"sections\":[{\"title\":\"Introduction\",\"order\":0,\"level\":1}]}",
            "```",
            "",
            "# Introduction",
            "```bgraph-section",
            "{\"id\":\"11111111-1111-5111-8111-111111111111\",\"node_type\":\"Section\",\"location\":{\"semantic\":{\"path\":\"1\",\"depth\":1,\"breadcrumbs\":[\"Sample\",\"Introduction\"]},\"physical\":null},\"text_order\":0,\"token_count\":1}",
            "```",
            "",
            "First paragraph body.",
            "```bgraph-paragraph",
            "{\"id\":\"22222222-2222-5222-8222-222222222222\",\"node_type\":\"Paragraph\",\"location\":{\"semantic\":{\"path\":\"2\",\"depth\":1,\"breadcrumbs\":[\"Sample\"]},\"physical\":null},\"text_order\":1,\"token_count\":3}",
            "```",
            "",
            "```bgraph-header",
            "Running header",
            "{\"id\":\"33333333-3333-5333-8333-333333333333\",\"node_type\":\"Header\",\"location\":{\"semantic\":{\"path\":\"3\",\"depth\":1,\"breadcrumbs\":[\"Sample\"]},\"physical\":null},\"text_order\":2,\"token_count\":2}",
            "```",
            "",
            "```bgraph-footer",
            "Confidential",
            "{\"id\":\"44444444-4444-5444-8444-444444444444\",\"node_type\":\"Footer\",\"location\":{\"semantic\":{\"path\":\"4\",\"depth\":1,\"breadcrumbs\":[\"Sample\"]},\"physical\":null},\"text_order\":3,\"token_count\":1}",
            "```",
            "",
        ]
        .join("\n")
    }

    #[test]
    fn body_only_removes_all_bgraph_fences() {
        let md = sample_bgraph_md();
        let out = strip(&md, StripMode::BodyOnly).expect("strip OK");
        // No bgraph fences at all.
        assert!(
            !out.contains("```bgraph"),
            "body-only output must not contain any bgraph fence; got:\n{out}"
        );
        // Body content survives.
        assert!(
            out.contains("# Introduction"),
            "Section body (heading) should survive body-only strip; got:\n{out}"
        );
        assert!(
            out.contains("First paragraph body."),
            "Paragraph body should survive body-only strip; got:\n{out}"
        );
        // Header/Footer running text (which lives inside the fence) is
        // removed.
        assert!(
            !out.contains("Running header"),
            "Header body lives inside fence and must NOT survive body-only strip"
        );
        assert!(
            !out.contains("Confidential"),
            "Footer body lives inside fence and must NOT survive body-only strip"
        );
    }

    #[test]
    fn keep_metadata_preserves_doc_level_block_and_bodies() {
        let md = sample_bgraph_md();
        let out = strip(&md, StripMode::KeepMetadata).expect("strip OK");
        // Doc-level survives — `bgraph` (no dash) is the
        // unstripped tag.
        assert!(
            out.contains("```bgraph\n"),
            "doc-level bgraph fence must survive keep-metadata; got:\n{out}"
        );
        assert!(
            out.contains("\"graph_sha256\""),
            "doc-level JSON line must survive keep-metadata"
        );
        // Bookmarks (dashed fence) is stripped.
        assert!(
            !out.contains("```bgraph-bookmarks"),
            "bookmarks fence must be stripped under keep-metadata"
        );
        // Per-element fences are stripped.
        assert!(
            !out.contains("```bgraph-section"),
            "section fence must be stripped under keep-metadata"
        );
        assert!(
            !out.contains("```bgraph-paragraph"),
            "paragraph fence must be stripped under keep-metadata"
        );
        // Body content survives.
        assert!(out.contains("# Introduction"));
        assert!(out.contains("First paragraph body."));
        // Header/Footer/Margin bodies (inside-fence) are stripped.
        assert!(!out.contains("Running header"));
        assert!(!out.contains("Confidential"));
    }

    #[test]
    fn noise_only_strips_header_footer_margin_only() {
        let md = sample_bgraph_md();
        let out = strip(&md, StripMode::NoiseOnly).expect("strip OK");
        // Doc-level + bookmarks + section/paragraph all survive.
        assert!(out.contains("```bgraph\n"));
        assert!(out.contains("```bgraph-bookmarks"));
        assert!(out.contains("```bgraph-section"));
        assert!(out.contains("```bgraph-paragraph"));
        // Header/Footer fences and their inside-fence bodies are gone.
        assert!(
            !out.contains("```bgraph-header"),
            "header fence must be stripped under noise-only"
        );
        assert!(
            !out.contains("```bgraph-footer"),
            "footer fence must be stripped under noise-only"
        );
        assert!(!out.contains("Running header"));
        assert!(!out.contains("Confidential"));
        // Body text remains.
        assert!(out.contains("# Introduction"));
        assert!(out.contains("First paragraph body."));
    }

    #[test]
    fn body_only_preserves_non_bgraph_code_blocks() {
        // A Paragraph body that itself contains a non-bgraph code
        // block (e.g., a ```rust ... ``` example) must survive
        // unchanged. The line scanner only recognizes ``` `bgraph* `
        // at line-start as a fence.
        let md = "\
```bgraph
{\"schema\":\"1.0.0\",\"graph_sha256\":\"x\"}
```

Body text with a code sample:
```rust
fn main() {}
```
End of body.
```bgraph-paragraph
{\"id\":\"x\",\"text_order\":0}
```
";
        let out = strip(md, StripMode::BodyOnly).expect("strip OK");
        assert!(
            out.contains("```rust"),
            "non-bgraph code block must survive body-only strip; got:\n{out}"
        );
        assert!(
            out.contains("fn main() {}"),
            "code block body must survive; got:\n{out}"
        );
        assert!(
            !out.contains("```bgraph"),
            "all bgraph fences must be removed; got:\n{out}"
        );
    }

    #[test]
    fn unterminated_fence_errors() {
        let md = "```bgraph\n{\"schema\":\"1.0.0\"}\n";
        // No closing ```; should be MalformedFence.
        match strip(md, StripMode::BodyOnly) {
            Err(ParseError::MalformedFence(_)) => {}
            other => panic!("expected MalformedFence for unterminated input, got {other:?}"),
        }
    }

    #[test]
    fn empty_input_returns_empty() {
        let out = strip("", StripMode::BodyOnly).expect("strip OK");
        assert!(out.is_empty());
    }

    #[test]
    fn input_with_no_bgraph_fences_passes_through_verbatim() {
        let md = "# Plain heading\n\nPlain prose.\n\n```rust\nfn main() {}\n```\n";
        let out = strip(md, StripMode::BodyOnly).expect("strip OK");
        assert_eq!(out, md, "plain markdown should pass through verbatim");
    }
}

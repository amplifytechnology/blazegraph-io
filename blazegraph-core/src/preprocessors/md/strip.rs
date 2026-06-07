//! bgraph fence stripping — bgraph.md → plain markdown / partial bgraph.md.
//!
//! Removes bgraph fence regions per a [`StripMode`]:
//! - [`StripMode::BodyWithFrontmatter`] (default): strip every bgraph
//!   fence and lift the doc-level `bgraph` block to YAML frontmatter at
//!   the top of the output. Produces docling-comparable plain markdown
//!   with provenance preserved.
//! - [`StripMode::BodyOnly`]: strip every bgraph fence, drop all
//!   metadata. Unstructured-equivalent body-only prose.
//! - [`StripMode::NodeTypes`]: apply the spec's structural rule for
//!   content boundaries to remove the listed element types entirely
//!   (body-above + fence pair). Non-matching bgraph fences pass through
//!   verbatim.
//!
//! Under v2.0.0 body-outside conventions, every mode preserves the
//! body content of unfiltered elements verbatim.
//!
//! Wire-format spec is the source of truth:
//! `docs/P2/core/architecture/08-bgraph-md-format.md` (v2.0.0).
//!
//! Reuses [`super::bgraph_md::bgraph_fence_open_tag`] and
//! [`super::bgraph_md::is_bare_fence_close`] (both `pub(super)`) so
//! the strip and round-trip parser share fence-recognition rules.

use super::bgraph_md::{bgraph_fence_open_tag, is_bare_fence_close, strip_trailing_newline};
use super::types::{ParseError, StripMode};

/// Strip bgraph fences from `input` according to `mode`.
///
/// Returns the stripped string. See [`StripMode`] for the variants.
///
/// Errors:
/// - [`ParseError::MalformedFence`] if the input contains an
///   unterminated bgraph fence (a `` ```bgraph* `` line at column zero
///   without a matching `` ``` `` close before EOF).
/// - [`ParseError::JsonParse`] if [`StripMode::BodyWithFrontmatter`]
///   was requested and the doc-level `bgraph` block's JSON failed to
///   parse during frontmatter lift.
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
    match mode {
        StripMode::BodyOnly => strip_all_fences(input),
        StripMode::BodyWithFrontmatter => strip_with_frontmatter(input),
        StripMode::NodeTypes(tags) => strip_with_node_types(input, &tags),
    }
}

/// Strip every bgraph fence from `input`. The body content of every
/// content variant (v2.0.0 body-outside) survives.
fn strip_all_fences(input: &str) -> Result<String, ParseError> {
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
            // Consume body lines through the fence close.
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

/// Strip every bgraph fence and lift the doc-level `bgraph` block to
/// YAML frontmatter at the top.
///
/// Output shape (one blank line after the closing `---`):
///
/// ```text
/// ---
/// <yaml>
/// ---
///
/// <stripped markdown body>
/// ```
///
/// Marcus's call (CR-55): lift the whole doc-level JSON as-is; do not
/// curate fields. The metadata contract is "whatever is in the
/// doc-level block." Future S4 metadata-normalization work can
/// crystallize the universal vs channel-specific split.
///
/// If the doc-level JSON happens to carry a top-level `bookmarks` field
/// (it does not in the current emitter — bookmarks live in a separate
/// `bgraph-outline` fence — but this is a forward-compat safety net),
/// that field is dropped. Bookmarks themselves are stripped in the
/// body-pass (separate fence).
fn strip_with_frontmatter(input: &str) -> Result<String, ParseError> {
    // 1. Body pass: strip every bgraph fence.
    let body = strip_all_fences(input)?;
    // 2. Frontmatter lift: parse the doc-level `bgraph` block's JSON
    //    payload. If the input has no doc-level block (or it's
    //    malformed), this returns `None` / `Err`.
    let Some(json_line) = extract_doc_level_json_line(input)? else {
        // No doc-level block detected. Return the body alone — produces
        // body-only output; the input wasn't a bgraph.md round-trip
        // artifact in the first place.
        return Ok(body);
    };
    let mut value: serde_json::Value =
        serde_json::from_str(json_line).map_err(|source| ParseError::JsonParse { source })?;
    if let Some(obj) = value.as_object_mut() {
        // Forward-compat: drop `bookmarks` if it surfaces at top level.
        obj.remove("bookmarks");
    }
    // Reshape into the canonical key order (title first, identity grouped at
    // the bottom). The frontmatter is the human-readable surface — `serde_yaml`
    // would otherwise emit keys in `serde_json::Map`'s default order (alphabetical
    // via `BTreeMap`), scattering related fields. The canonical reorder is
    // strip-layer-only; canonical bgraph.md keeps the JSON wire format intact.
    let ordered = canonical_doc_level_ordering(value);
    let yaml = serde_yaml::to_string(&ordered).map_err(|e| {
        ParseError::MalformedFence(format!(
            "failed to serialize doc-level block to YAML frontmatter: {e}"
        ))
    })?;
    // `serde_yaml::to_string` ends with `\n`. The frontmatter shape is:
    //   ---\n<yaml ending in \n>---\n\n<body>
    let yaml_trimmed = yaml.trim_end_matches('\n');
    let mut out = String::with_capacity(yaml.len() + body.len() + 16);
    out.push_str("---\n");
    out.push_str(yaml_trimmed);
    out.push_str("\n---\n\n");
    // If body starts with a blank line, drop one (so the separator is
    // exactly one blank). The body-only strip already compacts
    // post-fence blanks; for the doc-level fence specifically it leaves
    // a single leading blank we want to absorb into the frontmatter
    // separator.
    let body_view = body.strip_prefix('\n').unwrap_or(&body);
    out.push_str(body_view);
    Ok(out)
}

/// Reshape the parsed doc-level `bgraph` block into the canonical key order
/// for human-readable frontmatter:
///
/// 1. `title` — most relevant content metadata first.
/// 2. `schema`, `blazegraph_version`, `flow_type` — format / pipeline trait.
/// 3. `source` — provenance (nested map; inner keys keep `serde_json::Map`
///    insertion order, which is alphabetical: filename, format, sha256).
/// 4. `config_hash`, `graph_sha256` — hashes grouped at the bottom.
///
/// Any unknown top-level keys (forward-compat against future doc-level fields)
/// are appended after the known set in alphabetical order, so a future doc-level
/// addition surfaces deterministically without code change.
///
/// This is a strip-layer concern only. Canonical bgraph.md / bgraph.json keep
/// the JSON wire-format key order from the emitter; the reorder applies only
/// when lifting to YAML frontmatter for human readability.
fn canonical_doc_level_ordering(value: serde_json::Value) -> serde_yaml::Value {
    const CANONICAL_KEYS: &[&str] = &[
        "title",
        "schema",
        "blazegraph_version",
        "flow_type",
        "source",
        "config_hash",
        "graph_sha256",
    ];

    let serde_json::Value::Object(obj) = value else {
        // Non-object payload — pass through to serde_yaml as-is (shouldn't
        // happen for a well-formed doc-level block, but we don't crash).
        return serde_yaml::to_value(value).unwrap_or(serde_yaml::Value::Null);
    };

    let mut mapping = serde_yaml::Mapping::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for &key in CANONICAL_KEYS {
        if let Some(v) = obj.get(key) {
            let yaml_v = serde_yaml::to_value(v).unwrap_or(serde_yaml::Value::Null);
            mapping.insert(serde_yaml::Value::String(key.to_string()), yaml_v);
            seen.insert(key);
        }
    }

    let mut remaining: Vec<&String> = obj.keys().filter(|k| !seen.contains(k.as_str())).collect();
    remaining.sort();
    for key in remaining {
        if let Some(v) = obj.get(key) {
            let yaml_v = serde_yaml::to_value(v).unwrap_or(serde_yaml::Value::Null);
            mapping.insert(serde_yaml::Value::String(key.to_string()), yaml_v);
        }
    }

    serde_yaml::Value::Mapping(mapping)
}

/// Extract the JSON payload line of the input's doc-level `bgraph`
/// fence (the line between `` ```bgraph `` and `` ``` ``).
///
/// Returns `Ok(None)` if the input has no doc-level block (e.g., not a
/// bgraph.md artifact). Returns `Err(MalformedFence)` if a doc-level
/// fence opens but isn't closed before EOF.
fn extract_doc_level_json_line(input: &str) -> Result<Option<&str>, ParseError> {
    let mut iter = input.split_inclusive('\n');
    // Scan to the first `` ```bgraph `` line (skipping leading blanks).
    while let Some(line) = iter.next() {
        let trimmed = strip_trailing_newline(line);
        if trimmed.trim().is_empty() {
            continue;
        }
        if trimmed == "```bgraph" {
            // Doc-level block opened. The next line is the JSON
            // payload; the line after that should be the fence close.
            let Some(json_line) = iter.next() else {
                return Err(ParseError::MalformedFence(
                    "doc-level bgraph fence opened with no body".to_string(),
                ));
            };
            let json_trimmed = strip_trailing_newline(json_line);
            let Some(close_line) = iter.next() else {
                return Err(ParseError::MalformedFence(
                    "doc-level bgraph fence not closed".to_string(),
                ));
            };
            let close_trimmed = strip_trailing_newline(close_line);
            if !is_bare_fence_close(close_trimmed) {
                return Err(ParseError::MalformedFence(format!(
                    "doc-level bgraph fence has multi-line body or missing close; \
                     expected '```' immediately after JSON line, got: {close_trimmed:?}"
                )));
            }
            return Ok(Some(json_trimmed));
        }
        // First non-blank line isn't the doc-level fence — bail out.
        return Ok(None);
    }
    Ok(None)
}

/// Apply the spec's [Structural rule for content boundaries] to remove
/// every element whose per-element fence tag is in `tags`. Non-matching
/// bgraph fences pass through verbatim.
///
/// For each matching `` ```bgraph-<tag> `` fence open at line `i`:
/// - **body_start**: walk from `i-1` down to 0. Stop at the first line
///   that is (a) blank, (b) a bare ``` ``` `` fence close, or (c)
///   start-of-file. `body_start = boundary + 1`.
/// - **fence_close**: walk from `i+1` up. Stop at the first bare
///   ``` ``` `` line. If none found → [`ParseError::MalformedFence`].
/// - The inclusive range `[body_start, fence_close]` is marked for
///   deletion.
///
/// After all matches are identified, the output is the lines not in
/// any deletion range. Blank-line compaction (one blank max across a
/// deletion boundary) follows the same discipline as
/// [`strip_all_fences`].
///
/// [Structural rule for content boundaries]:
/// https://github.com/AmplifyTechnology/blazegraph-io-app/blob/main/docs/P2/core/architecture/08-bgraph-md-format.md#structural-rule-for-content-boundaries
fn strip_with_node_types(input: &str, tags: &[String]) -> Result<String, ParseError> {
    // Split into line views (without trailing newlines) for boundary
    // analysis, but keep the original `split_inclusive` slices so we
    // can re-emit byte-identically.
    let lines: Vec<&str> = input.split_inclusive('\n').collect();
    // Pre-compute trimmed (no trailing \n / \r) views.
    let trimmed: Vec<&str> = lines.iter().map(|l| strip_trailing_newline(l)).collect();

    let n = lines.len();
    let mut delete: Vec<bool> = vec![false; n];
    // Track positions of bare fence closes for use as upper boundaries
    // ("body_start = first line after the most recent boundary above").
    // We scan strictly: a fence-open without a close before EOF is a
    // MalformedFence error.
    let mut i = 0usize;
    while i < n {
        let line = trimmed[i];
        let Some(tag) = bgraph_fence_open_tag(line) else {
            i += 1;
            continue;
        };
        // This is a bgraph-something fence open. Find its close.
        let mut close_idx = None;
        let mut j = i + 1;
        while j < n {
            if is_bare_fence_close(trimmed[j]) {
                close_idx = Some(j);
                break;
            }
            j += 1;
        }
        let Some(close_idx) = close_idx else {
            return Err(ParseError::MalformedFence(format!(
                "unterminated bgraph fence with tag {tag:?} during strip_with_node_types"
            )));
        };
        // Only delete if the per-element tag matches one of the
        // requested types. `bgraph_fence_open_tag` returns the full
        // info-string (e.g., "bgraph-header"); the user supplies the
        // un-prefixed type (e.g., "header"). Strip the `bgraph-`
        // prefix before comparing. The bare `bgraph` doc-level tag
        // never matches (no dash); the CLI also rejects `bgraph` as
        // a node-type target.
        let element_type = tag.strip_prefix("bgraph-");
        let matches = match element_type {
            Some(et) => tags.iter().any(|t| t == et),
            None => false, // doc-level `bgraph` fence — never a node-type target
        };
        if matches {
            // body_start: walk back from i-1 to find the boundary.
            let body_start = if i == 0 {
                0
            } else {
                let mut k: isize = (i as isize) - 1;
                let mut boundary: isize = -1;
                while k >= 0 {
                    let kt = trimmed[k as usize];
                    if kt.trim().is_empty() || is_bare_fence_close(kt) {
                        boundary = k;
                        break;
                    }
                    k -= 1;
                }
                (boundary + 1) as usize
            };
            for k in body_start..=close_idx {
                delete[k] = true;
            }
        }
        // Advance past the close, regardless of whether we deleted.
        i = close_idx + 1;
    }

    // Now emit the kept lines, applying the same orphan-blank-line
    // compaction as `strip_all_fences`.
    let mut out = String::with_capacity(input.len());
    let mut last_emitted_blank = false;
    let mut just_stripped = false;
    let mut idx = 0usize;
    while idx < n {
        if delete[idx] {
            // The next non-deleted line is "after a stripped range" —
            // mark just_stripped so we can absorb an orphan blank.
            // Advance through the entire deletion range so we don't
            // re-enter this branch on each deleted line.
            while idx < n && delete[idx] {
                idx += 1;
            }
            just_stripped = true;
            continue;
        }
        let t = trimmed[idx];
        let is_blank = t.trim().is_empty();
        if just_stripped && is_blank {
            just_stripped = false;
            idx += 1;
            continue;
        }
        if is_blank && last_emitted_blank && just_stripped {
            just_stripped = false;
            idx += 1;
            continue;
        }
        out.push_str(lines[idx]);
        last_emitted_blank = is_blank;
        just_stripped = false;
        idx += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal v2.0.0 emitter-style bgraph.md sample.
    /// Header/Footer body live *outside* the fence per convention C-3.
    fn sample_bgraph_md() -> String {
        [
            "```bgraph",
            "{\"schema\":\"2.0.0\",\"blazegraph_version\":\"0.6.0\",\"source\":{\"format\":\"pdf\",\"filename\":\"x.pdf\",\"sha256\":\"src-sha\"},\"flow_type\":\"Fixed\",\"title\":\"Sample\",\"config_hash\":\"cfg-sha\",\"graph_sha256\":\"deadbeef\"}",
            "```",
            "",
            "```bgraph-outline",
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
            "Running header",
            "```bgraph-header",
            "{\"id\":\"33333333-3333-5333-8333-333333333333\",\"node_type\":\"Header\",\"location\":{\"semantic\":{\"path\":\"3\",\"depth\":1,\"breadcrumbs\":[\"Sample\"]},\"physical\":null},\"text_order\":2,\"token_count\":2}",
            "```",
            "",
            "Confidential",
            "```bgraph-footer",
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
        // All body content (including v2.0.0 body-outside H/F) survives.
        for body in &[
            "# Introduction",
            "First paragraph body.",
            "Running header",
            "Confidential",
        ] {
            assert!(
                out.contains(body),
                "body content {body:?} must survive body-only strip under v2.0.0; got:\n{out}"
            );
        }
    }

    #[test]
    fn body_only_preserves_non_bgraph_code_blocks() {
        // A Paragraph body that itself contains a non-bgraph code
        // block (e.g., a ```rust ... ``` example) must survive
        // unchanged. The line scanner only recognizes ``` `bgraph* `
        // at line-start as a fence.
        let md = "\
```bgraph
{\"schema\":\"2.0.0\",\"graph_sha256\":\"x\"}
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
        let md = "```bgraph\n{\"schema\":\"2.0.0\"}\n";
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

    #[test]
    fn strip_recognizes_codeblock_list_blockquote_table_fences() {
        // Amendment F (B6) variants — body-outside content like the
        // rest. body-only strips every bgraph fence; body survives.
        let md = "\
```bgraph
{\"schema\":\"2.0.0\",\"graph_sha256\":\"x\"}
```

- bullet one
- bullet two
```bgraph-list
{\"id\":\"x\",\"text_order\":0}
```

> a quote
```bgraph-blockquote
{\"id\":\"y\",\"text_order\":1}
```

| a | b |
|---|---|
| 1 | 2 |
```bgraph-table
{\"id\":\"z\",\"text_order\":2}
```

```rust
fn main() {}
```
```bgraph-codeblock
{\"id\":\"w\",\"text_order\":3}
```
";

        // body-only: every bgraph fence vanishes.
        let body_only = strip(md, StripMode::BodyOnly).expect("body-only OK");
        for tag in [
            "```bgraph",
            "```bgraph-list",
            "```bgraph-blockquote",
            "```bgraph-table",
            "```bgraph-codeblock",
        ] {
            assert!(
                !body_only.contains(tag),
                "body-only should strip `{tag}`; got:\n{body_only}"
            );
        }
        // Bodies (outside-fence) survive.
        assert!(body_only.contains("- bullet one"));
        assert!(body_only.contains("> a quote"));
        assert!(body_only.contains("| a | b |"));
        assert!(body_only.contains("fn main() {}"));
    }

    // ====================================================================
    // CR-55 — Test plan
    // ====================================================================
    //
    // The 14 tests below pin the CR-55 surface. Test numbers map 1:1 to
    // the "Test plan / Unit" section in
    // docs/P2/core/change-requests/CR-55-strip-node-types-cli.md.

    /// CR-55 Test 1: default mode produces YAML frontmatter + plain body.
    /// Pin: output starts with `---\n`, contains parseable YAML round-
    /// tripping the doc-level JSON, closes with `---\n\n` (exactly one
    /// blank line), then plain markdown body.
    #[test]
    fn cr55_test1_default_mode_emits_yaml_frontmatter_and_plain_body() {
        let md = sample_bgraph_md();
        let out = strip(&md, StripMode::BodyWithFrontmatter).expect("strip OK");
        assert!(
            out.starts_with("---\n"),
            "output must start with `---\\n`; got:\n{out}"
        );
        // Locate the closing `---\n\n`.
        let after_open = &out[4..]; // skip the opening `---\n`
        let close_idx = after_open
            .find("\n---\n")
            .expect("must contain closing `---` separator");
        let yaml_payload = &after_open[..close_idx];
        let yaml_payload_with_nl = format!("{yaml_payload}\n");
        // YAML round-trips.
        let parsed: serde_json::Value =
            serde_yaml::from_str(&yaml_payload_with_nl).expect("frontmatter YAML must parse");
        assert_eq!(
            parsed.get("graph_sha256").and_then(|v| v.as_str()),
            Some("deadbeef")
        );
        assert_eq!(parsed.get("schema").and_then(|v| v.as_str()), Some("2.0.0"));
        // Exactly one blank line between `---` close and body.
        // After `\n---\n` (the close), the next two bytes should be
        // `\n<non-blank>` — i.e., a single blank then content.
        let after_close = &after_open[close_idx + "\n---\n".len()..];
        assert!(
            after_close.starts_with('\n'),
            "exactly one blank line required between frontmatter and body; got after close:\n{after_close:?}"
        );
        let body = &after_close[1..];
        assert!(
            !body.starts_with('\n'),
            "must be exactly one blank line, not two+; got:\n{after_close:?}"
        );
        // Body content survives.
        assert!(body.contains("# Introduction"));
        assert!(body.contains("First paragraph body."));
        // No bgraph fences in body.
        assert!(
            !body.contains("```bgraph"),
            "no bgraph fences allowed in body; got:\n{body}"
        );
    }

    /// CR-55 Test 1b: canonical key ordering in the lifted frontmatter —
    /// `title` first, identity hashes (`config_hash`, `graph_sha256`) grouped
    /// at the bottom, `source` (nested provenance) just before them. The
    /// reorder is strip-layer-only; canonical bgraph.md keeps the JSON
    /// wire-format key order.
    #[test]
    fn cr55_canonical_key_ordering_in_frontmatter() {
        let md = sample_bgraph_md();
        let out = strip(&md, StripMode::BodyWithFrontmatter).expect("strip OK");
        let frontmatter = out
            .strip_prefix("---\n")
            .and_then(|s| s.split_once("\n---\n"))
            .map(|(yaml, _)| yaml)
            .expect("frontmatter delimited");
        let expected_order = [
            "title:",
            "schema:",
            "blazegraph_version:",
            "flow_type:",
            "source:",
            "config_hash:",
            "graph_sha256:",
        ];
        let mut last_idx: Option<usize> = None;
        for key in expected_order {
            let idx = frontmatter.find(key).unwrap_or_else(|| {
                panic!("canonical key {key} missing in frontmatter:\n{frontmatter}")
            });
            if let Some(prev) = last_idx {
                assert!(
                    idx > prev,
                    "canonical ordering violated: `{key}` appears before previous key\n{frontmatter}"
                );
            }
            last_idx = Some(idx);
        }
    }

    /// CR-55 Test 2: YAML preserves nested structures (source: {format,
    /// filename, sha256} stays as a nested map, not a flattened string).
    #[test]
    fn cr55_test2_frontmatter_preserves_nested_source_block() {
        let md = sample_bgraph_md();
        let out = strip(&md, StripMode::BodyWithFrontmatter).expect("strip OK");
        // Extract frontmatter between the two `---` lines.
        let frontmatter = out
            .strip_prefix("---\n")
            .and_then(|s| s.split_once("\n---\n"))
            .map(|(yaml, _rest)| yaml)
            .expect("frontmatter must be delimited");
        let yaml_with_nl = format!("{frontmatter}\n");
        let parsed: serde_json::Value = serde_yaml::from_str(&yaml_with_nl).expect("YAML parses");
        let source = parsed.get("source").expect("source key present");
        assert!(
            source.is_object(),
            "source must be a nested map, got: {source:?}"
        );
        assert_eq!(
            source.get("format").and_then(|v| v.as_str()),
            Some("pdf"),
            "nested source.format preserved"
        );
        assert_eq!(
            source.get("filename").and_then(|v| v.as_str()),
            Some("x.pdf"),
            "nested source.filename preserved"
        );
        assert_eq!(
            source.get("sha256").and_then(|v| v.as_str()),
            Some("src-sha"),
            "nested source.sha256 preserved"
        );
    }

    /// CR-55 Test 3: bookmarks dropped under default mode.
    /// The `bgraph-outline` fence content is stripped from the body;
    /// no `bookmarks:` key in frontmatter; no orphan blank line.
    #[test]
    fn cr55_test3_bookmarks_dropped_in_default_mode() {
        let md = sample_bgraph_md();
        let out = strip(&md, StripMode::BodyWithFrontmatter).expect("strip OK");
        // No `bookmarks:` key in the frontmatter (the emitter doesn't
        // put bookmarks in the doc-level block, but verify forward-
        // compat safety net).
        let frontmatter = out
            .strip_prefix("---\n")
            .and_then(|s| s.split_once("\n---\n"))
            .map(|(yaml, _)| yaml)
            .expect("frontmatter delimited");
        assert!(
            !frontmatter.contains("bookmarks:"),
            "frontmatter must not contain bookmarks key; got:\n{frontmatter}"
        );
        // No `bgraph-outline` fence body in the document body
        // (sample's bookmarks JSON line carries the literal
        // `"sections":[{"title":"Introduction"`...).
        assert!(
            !out.contains("\"sections\":[{\"title\":\"Introduction\""),
            "bgraph-outline fence body must be stripped from output"
        );
        // No no-blank-line orphan double-blanks where the bookmarks
        // fence used to live (between doc-level and first paragraph
        // body). A robust check: no `\n\n\n` runs.
        assert!(
            !out.contains("\n\n\n"),
            "no triple-blank runs (would indicate orphan blank after stripped bookmarks); got:\n{out}"
        );
    }

    /// CR-55 Test 4: `body-only` matches pre-CR-55 behavior. Subsumed
    /// by `body_only_removes_all_bgraph_fences` above — pinned here
    /// explicitly for the CR test-plan numbering.
    #[test]
    fn cr55_test4_body_only_unchanged_from_pre_cr55() {
        let md = sample_bgraph_md();
        let out = strip(&md, StripMode::BodyOnly).expect("strip OK");
        // Same shape as the pre-CR-55 test: no bgraph fences, all
        // body content present.
        assert!(!out.contains("```bgraph"));
        assert!(out.contains("# Introduction"));
        assert!(out.contains("Running header"));
        assert!(out.contains("Confidential"));
    }

    /// CR-55 Test 5: the pre-CR-55 metadata-retaining variant is gone.
    /// The variant's removal is evidenced by (a) this test compiling
    /// against an exhaustive list of post-CR-55 variants and (b)
    /// `grep -r` for the variant name returning nothing across the
    /// submodule.
    #[test]
    fn cr55_test5_pre_cr55_metadata_variant_is_removed() {
        // Exhaustive variant list — adding a new variant must update
        // this match. The pre-CR-55 inline-metadata variant has been
        // deleted.
        let tags: Vec<String> = vec![];
        let modes = [
            StripMode::BodyOnly,
            StripMode::BodyWithFrontmatter,
            StripMode::NodeTypes(tags),
        ];
        // Each compiles & strip is callable.
        for m in modes {
            let _ = strip("", m).expect("empty strip OK");
        }
    }

    /// CR-55 Test 6: `--node-types` alone (no --mode override)
    /// applied through the lib API: input with one bgraph-header
    /// element. After `NodeTypes(["header"])`, the header body + fence
    /// are both removed (the CLI's two-pass composition with the
    /// default mode is tested at the CLI layer; here we exercise the
    /// lib API which applies only the structural-rule pass).
    #[test]
    fn cr55_test6_node_types_lib_removes_matching_element_body_and_fence() {
        let md = sample_bgraph_md();
        let out = strip(&md, StripMode::NodeTypes(vec!["header".to_string()])).expect("strip OK");
        assert!(
            !out.contains("```bgraph-header"),
            "header fence must be removed; got:\n{out}"
        );
        assert!(
            !out.contains("Running header"),
            "header body-above must be removed; got:\n{out}"
        );
        // Non-header content is preserved verbatim, including other
        // fences (this is lib-API behavior; the CLI composes with a
        // mode pass to also strip them).
        assert!(out.contains("First paragraph body."));
        assert!(out.contains("```bgraph-paragraph"));
        assert!(out.contains("Confidential")); // footer body preserved
    }

    /// CR-55 Test 7: `--node-types` composed with `--mode body-only`.
    /// This is CLI-layer composition; we model it as two lib calls
    /// (structural-rule pass then body-only pass) and verify the
    /// final shape.
    #[test]
    fn cr55_test7_node_types_composed_with_body_only_pass() {
        let md = sample_bgraph_md();
        let after_filter =
            strip(&md, StripMode::NodeTypes(vec!["header".to_string()])).expect("filter pass OK");
        let out = strip(&after_filter, StripMode::BodyOnly).expect("body-only pass OK");
        // No frontmatter under body-only.
        assert!(
            !out.starts_with("---\n"),
            "body-only composition must not emit frontmatter; got:\n{out}"
        );
        // Header is gone.
        assert!(!out.contains("Running header"));
        // Other body content survives.
        assert!(out.contains("# Introduction"));
        assert!(out.contains("First paragraph body."));
        assert!(out.contains("Confidential"));
        // No bgraph fences anywhere.
        assert!(!out.contains("```bgraph"));
    }

    /// CR-55 Test 8: header at start of file. The bgraph-header
    /// element is the literal first per-element fence (no preceding
    /// blank). `--node-types header` strips cleanly without leaving
    /// an orphan blank.
    #[test]
    fn cr55_test8_header_at_start_of_file_strips_cleanly() {
        let md = "\
Running header
```bgraph-header
{\"id\":\"x\",\"text_order\":0}
```

Body paragraph.
```bgraph-paragraph
{\"id\":\"y\",\"text_order\":1}
```
";
        let out = strip(md, StripMode::NodeTypes(vec!["header".to_string()])).expect("strip OK");
        assert!(!out.contains("Running header"));
        assert!(!out.contains("```bgraph-header"));
        // Paragraph element survives.
        assert!(out.contains("Body paragraph."));
        assert!(out.contains("```bgraph-paragraph"));
        // No orphan leading blank.
        assert!(
            !out.starts_with('\n'),
            "no leading orphan blank; got:\n{out:?}"
        );
    }

    /// CR-55 Test 9: empty body case. Fence with no body-above (only a
    /// blank line above). The fence pair is removed; the walk does NOT
    /// over-reach into the previous element.
    #[test]
    fn cr55_test9_empty_body_strips_fence_pair_only() {
        // Section element above with body "previous body"; a
        // hypothetical empty-body header below (body-start lands on
        // the fence-open line itself because the previous line is
        // blank).
        let md = "\
previous body
```bgraph-section
{\"id\":\"s\",\"text_order\":0}
```

```bgraph-header
{\"id\":\"h\",\"text_order\":1}
```
";
        let out = strip(md, StripMode::NodeTypes(vec!["header".to_string()])).expect("strip OK");
        // Header fence pair gone.
        assert!(!out.contains("```bgraph-header"));
        // Previous element fully intact.
        assert!(out.contains("previous body"));
        assert!(out.contains("```bgraph-section"));
        // The fence-close of the previous section still in output.
        assert!(out.contains("\"text_order\":0"));
    }

    /// CR-55 Test 10: reserved-prefix escape inside codeblock — a
    /// bgraph-paragraph body containing the literal `` ```bgraph-header ``
    /// inside an escaped codeblock. `--node-types header` must NOT
    /// treat that inside-body literal as a strip target. The
    /// `bgraph_fence_open_tag` discipline recognizes only column-zero
    /// `` ```bgraph* `` fences; the spec's escape convention writes
    /// `` \```bgraph `` inside bodies (which is not recognized as a
    /// fence). We simulate a body line with leading whitespace + the
    /// literal token to ensure the line scanner does not match.
    #[test]
    fn cr55_test10_reserved_prefix_in_body_not_stripped() {
        let md = "\
A paragraph that mentions the reserved prefix in its body:
    ```bgraph-header
(indented — not at column zero, so not a fence)
```bgraph-paragraph
{\"id\":\"p\",\"text_order\":0}
```
";
        // The indented `   ```bgraph-header` line is NOT a fence
        // (not at column zero). Asking to strip headers must therefore
        // leave the paragraph's body intact.
        let out = strip(md, StripMode::NodeTypes(vec!["header".to_string()])).expect("strip OK");
        assert!(
            out.contains("mentions the reserved prefix"),
            "paragraph body must survive when reserved-prefix appears only mid-body; got:\n{out}"
        );
        assert!(
            out.contains("```bgraph-paragraph"),
            "paragraph fence must survive (not a header tag); got:\n{out}"
        );
    }

    // Tests 11 + 12 (CLI clap-level rejection of unknown tags / `bgraph`)
    // live in `blazegraph-io/blazegraph-cli/tests/cli_roundtrip.rs`.
    // They're not implementable as `strip.rs` unit tests because the
    // rejection happens at clap's `value_parser` layer before the lib is
    // called. The lib's `NodeTypes(Vec<String>)` accepts any strings;
    // tag validity is a CLI-layer concern.

    /// CR-55 Test 13: unterminated fence error. Matches existing
    /// `unterminated_fence_errors` behavior — pinned here for the CR
    /// numbering, and also verified against the structural-rule pass.
    #[test]
    fn cr55_test13_unterminated_fence_errors_under_node_types() {
        let md = "\
Some body.
```bgraph-header
{\"id\":\"h\",\"text_order\":0}
";
        match strip(md, StripMode::NodeTypes(vec!["header".to_string()])) {
            Err(ParseError::MalformedFence(_)) => {}
            other => panic!("expected MalformedFence; got {other:?}"),
        }
    }

    /// CR-55 Test 14: idempotence. Default-strip output run through
    /// strip again is approximately a no-op — the YAML frontmatter
    /// survives (it's not a bgraph fence; the strip ignores it), and
    /// the markdown body is unchanged. On the second pass with
    /// `BodyOnly`, the frontmatter is also untouched because it isn't
    /// a bgraph fence.
    #[test]
    fn cr55_test14_default_output_is_idempotent_under_repeated_strip() {
        let md = sample_bgraph_md();
        let first = strip(&md, StripMode::BodyWithFrontmatter).expect("first strip");
        // Second pass: re-apply the same mode. The doc-level fence is
        // gone (already stripped), so frontmatter lift is a no-op (no
        // doc-level block to lift); the body-only pass is also a
        // no-op (no bgraph fences left).
        let second = strip(&first, StripMode::BodyWithFrontmatter).expect("second strip");
        assert_eq!(
            first, second,
            "default-strip output must be idempotent under re-stripping"
        );
        // Also verify the BodyOnly second pass is a no-op (frontmatter
        // is NOT a bgraph fence, so it survives).
        let body_only_second = strip(&first, StripMode::BodyOnly).expect("body-only second");
        assert_eq!(
            first, body_only_second,
            "BodyOnly second pass on default output is a no-op"
        );
    }
}

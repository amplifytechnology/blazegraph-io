//! YAML frontmatter pre-pass for the generic markdown channel.
//!
//! Recognizes the `---\n…\n---\n` block convention at the top of a
//! markdown file. Canonical fields land in their typed slots on
//! [`DocumentMetadata`]; everything else passes through as opaque
//! `serde_json::Value` in `DocumentMetadata.extras`.
//!
//! ## Lenient parsing
//!
//! Malformed YAML and "looks like frontmatter but isn't" inputs return
//! `(DocumentMetadata::default(), input)` unchanged — the `---` block
//! stays in the markdown body to be parsed as ordinary content (which
//! CommonMark renders as a horizontal rule plus paragraph plus
//! horizontal rule). The contract is "capture what exists; don't assume
//! correctness."
//!
//! ## YAML library boundary
//!
//! [`gray_matter`] handles both the `---` detection and the YAML
//! decoding via its internal YAML engine. The lib's result is a
//! `gray_matter::Pod`, which we convert immediately to
//! `serde_json::Value` so `DocumentMetadata.extras` and the canonical-
//! field deserialization never see a `gray_matter` type. The YAML lib
//! coupling lives entirely inside this module — swapping to a different
//! YAML engine is a one-file change.
//!
//! See the B6 AAR (`docs/P3/core/aars/2026-05-12-B6-...`) for the
//! handoff's `serde_norway` library lock and why `gray_matter` is
//! the actual implementation choice.

use crate::types::DocumentMetadata;
use gray_matter::engine::YAML;
use gray_matter::{Matter, Pod};
use std::collections::BTreeMap;

/// Maximum number of lines we'll scan for a closing `---` line before
/// giving up and treating the input as having no frontmatter. Real
/// frontmatter blocks are short (typically <30 lines); a `---` at the
/// top with no closer hundreds of lines later is almost certainly a
/// horizontal rule, not unterminated frontmatter.
const FRONTMATTER_CLOSER_SEARCH_LIMIT: usize = 200;

/// Detect, extract, and parse YAML frontmatter from a markdown string.
///
/// Returns `(metadata, body)` where:
/// - `metadata` carries the parsed frontmatter (canonical fields typed,
///   unknown fields in `extras`). [`DocumentMetadata::default`] when
///   there is no frontmatter or it fails to parse.
/// - `body` is the input slice with the frontmatter block (and its
///   trailing newline) stripped. The original input slice unchanged
///   when there's no frontmatter.
///
/// Lenient by design: malformed YAML returns the default metadata and
/// the unchanged input, so the `---` block is interpreted as ordinary
/// markdown by the downstream parser. This matches the design pass's
/// "no errors on malformed frontmatter — capture what exists; don't
/// assume correctness" rule.
pub fn extract_frontmatter(input: &str) -> (DocumentMetadata, &str) {
    let Some(closer_offset) = find_frontmatter_block(input) else {
        return (DocumentMetadata::default(), input);
    };

    // Parse via gray_matter. In 0.3.x, `parse` returns
    // `Result<ParsedEntity, gray_matter::Error>`; on YAML failure we
    // get `Err(_)`. ParsedEntity.data is `Option<Pod>` (None on empty
    // frontmatter block). We do not use `parse_with_struct` because
    // we need to keep all unknown keys in `extras` rather than
    // dropping them.
    let matter = Matter::<YAML>::new();
    let Ok(parsed) = matter.parse(input) else {
        // YAML parse failed → lenient: keep the input unchanged so
        // the `---` block is interpreted as markdown content.
        return (DocumentMetadata::default(), input);
    };

    let pod = parsed.data.unwrap_or(Pod::Null);

    let metadata = pod_to_metadata(&pod);

    // Slice off the frontmatter block + its trailing newline. We
    // computed the closer offset ourselves rather than using
    // `parsed.content` because the latter is owned and we want to
    // hand back a borrowed slice (the downstream parser doesn't need
    // a new allocation here).
    let body_start = closer_offset;
    let body = &input[body_start..];
    (metadata, body)
}

/// Locate the byte offset of the start of the markdown body that
/// follows a `---\n…\n---\n` frontmatter block at the top of `input`.
///
/// Returns `None` if `input` does not start with a frontmatter block
/// (no `---\n` opener, or no `---` closer within
/// [`FRONTMATTER_CLOSER_SEARCH_LIMIT`] lines).
///
/// **Detection rule.** Line 1 must be exactly `---` (with optional
/// trailing whitespace tolerated). Subsequent lines are scanned until
/// either (a) a line exactly matching `---`, in which case the body
/// starts on the next line, or (b) the search limit is exceeded, in
/// which case there is no frontmatter.
fn find_frontmatter_block(input: &str) -> Option<usize> {
    // Quick reject: must start with --- on line 1.
    let first_line_end = input.find('\n')?;
    let first_line = &input[..first_line_end];
    if first_line.trim_end() != "---" {
        return None;
    }

    // Scan subsequent lines for the closer.
    let mut cursor = first_line_end + 1;
    let mut lines_scanned = 0;
    while cursor < input.len() && lines_scanned < FRONTMATTER_CLOSER_SEARCH_LIMIT {
        let rest = &input[cursor..];
        let line_end = rest.find('\n').map(|n| cursor + n).unwrap_or(input.len());
        let line = &input[cursor..line_end];
        if line.trim_end() == "---" {
            // Body starts after this `---\n`.
            return Some(line_end + 1);
        }
        cursor = line_end + 1;
        lines_scanned += 1;
    }
    None
}

/// Project a `gray_matter::Pod` (the lib's untyped YAML representation)
/// onto `DocumentMetadata`. Canonical keys (`title`, `author`, `date`,
/// `tags`, `description`, `draft`) land in their typed fields; everything
/// else passes through to `extras` as `serde_json::Value`.
fn pod_to_metadata(pod: &Pod) -> DocumentMetadata {
    let mut metadata = DocumentMetadata::default();

    let Pod::Hash(map) = pod else {
        // Frontmatter that doesn't parse as a top-level map (e.g.,
        // just a bare string) — we have nothing to project. Lenient:
        // return defaults, the `---` block is still stripped.
        return metadata;
    };

    for (key, value) in map {
        match key.as_str() {
            "title" => metadata.title = pod_as_string(value),
            "author" => metadata.author = pod_as_string(value),
            "date" => metadata.date = pod_as_string(value),
            "description" => metadata.description = pod_as_string(value),
            "draft" => metadata.draft = pod_as_bool(value),
            "tags" => {
                if let Some(tags) = pod_as_string_array(value) {
                    metadata.tags = tags;
                }
            }
            other => {
                if let Some(json_value) = pod_to_json(value) {
                    metadata.extras.insert(other.to_string(), json_value);
                }
            }
        }
    }

    // Stable extras key ordering is guaranteed by `BTreeMap`'s
    // ordered iteration — required for the canonical JSON / graph_sha
    // invariant under round-trip.
    let _: &BTreeMap<_, _> = &metadata.extras;

    metadata
}

/// Coerce a `Pod` to a string if it's representable as one. Numbers
/// and booleans are stringified (frontmatter `date: 2026-05-12` parses
/// as YAML int/date, but a stringified rendering is the lossless
/// projection onto our free-form `Option<String>` field).
fn pod_as_string(pod: &Pod) -> Option<String> {
    match pod {
        Pod::String(s) => Some(s.clone()),
        Pod::Integer(i) => Some(i.to_string()),
        Pod::Float(f) => Some(f.to_string()),
        Pod::Boolean(b) => Some(b.to_string()),
        Pod::Null => None,
        _ => None,
    }
}

fn pod_as_bool(pod: &Pod) -> Option<bool> {
    match pod {
        Pod::Boolean(b) => Some(*b),
        _ => None,
    }
}

/// Coerce a `Pod::Array` of string-coercible scalars to `Vec<String>`.
/// Non-array shapes return None; non-string elements within an array
/// are stringified via `pod_as_string`.
fn pod_as_string_array(pod: &Pod) -> Option<Vec<String>> {
    let Pod::Array(items) = pod else {
        return None;
    };
    Some(items.iter().filter_map(pod_as_string).collect())
}

/// Convert a `Pod` to `serde_json::Value` for storage in `extras`.
/// Returns `None` only for shapes that cannot be projected at all
/// (which `Pod` doesn't have today; all variants are mappable).
fn pod_to_json(pod: &Pod) -> Option<serde_json::Value> {
    Some(match pod {
        Pod::Null => serde_json::Value::Null,
        Pod::String(s) => serde_json::Value::String(s.clone()),
        Pod::Integer(i) => serde_json::Value::Number((*i).into()),
        Pod::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Pod::Boolean(b) => serde_json::Value::Bool(*b),
        Pod::Array(items) => {
            let json_items: Vec<serde_json::Value> = items.iter().filter_map(pod_to_json).collect();
            serde_json::Value::Array(json_items)
        }
        Pod::Hash(map) => {
            // serde_json::Map preserves insertion order; for
            // determinism (graph_sha256 invariant) sort keys.
            let mut entries: Vec<(&String, &Pod)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let mut out = serde_json::Map::new();
            for (k, v) in entries {
                if let Some(jv) = pod_to_json(v) {
                    out.insert(k.clone(), jv);
                }
            }
            serde_json::Value::Object(out)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_frontmatter_parses_yaml_block_at_top() {
        let input = "---\ntitle: Hello\nauthor: Alice\n---\n# Heading\n\nBody.\n";
        let (meta, body) = extract_frontmatter(input);
        assert_eq!(meta.title.as_deref(), Some("Hello"));
        assert_eq!(meta.author.as_deref(), Some("Alice"));
        assert_eq!(body, "# Heading\n\nBody.\n");
    }

    #[test]
    fn extract_frontmatter_returns_default_when_no_opener() {
        let input = "# Heading\n\nBody.\n";
        let (meta, body) = extract_frontmatter(input);
        assert!(meta.title.is_none());
        assert_eq!(body, input);
    }

    #[test]
    fn extract_frontmatter_returns_default_when_no_closer() {
        let input = "---\ntitle: Stranded\n\nNo closer ever.\n";
        let (meta, body) = extract_frontmatter(input);
        assert!(meta.title.is_none());
        assert_eq!(body, input, "body should be unchanged on missing closer");
    }

    #[test]
    fn extract_frontmatter_canonical_fields_typed() {
        let input = "---\n\
                     title: My Doc\n\
                     author: Marcus\n\
                     date: 2026-05-12\n\
                     description: Long form\n\
                     draft: true\n\
                     tags: [rust, blazegraph, b6]\n\
                     ---\n\
                     Body.\n";
        let (meta, _body) = extract_frontmatter(input);
        assert_eq!(meta.title.as_deref(), Some("My Doc"));
        assert_eq!(meta.author.as_deref(), Some("Marcus"));
        assert_eq!(meta.date.as_deref(), Some("2026-05-12"));
        assert_eq!(meta.description.as_deref(), Some("Long form"));
        assert_eq!(meta.draft, Some(true));
        assert_eq!(
            meta.tags,
            vec![
                "rust".to_string(),
                "blazegraph".to_string(),
                "b6".to_string()
            ]
        );
        // No unknown keys → extras empty.
        assert!(
            meta.extras.is_empty(),
            "canonical-only input should produce empty extras; got {:?}",
            meta.extras
        );
    }

    #[test]
    fn extract_frontmatter_unknown_fields_go_to_extras() {
        let input = "---\n\
                     title: Doc\n\
                     custom_key: custom_value\n\
                     priority: 7\n\
                     ---\n\
                     Body.\n";
        let (meta, _body) = extract_frontmatter(input);
        assert_eq!(meta.title.as_deref(), Some("Doc"));
        assert_eq!(
            meta.extras.get("custom_key"),
            Some(&serde_json::Value::String("custom_value".to_string())),
        );
        assert_eq!(
            meta.extras.get("priority"),
            Some(&serde_json::Value::Number(7.into())),
        );
    }

    #[test]
    fn extract_frontmatter_malformed_yaml_is_lenient() {
        // YAML with mismatched braces — gray_matter returns
        // `data: None`. We treat this as "no frontmatter" and leave
        // the `---` block in the body verbatim.
        let input = "---\ntitle: [unclosed\n---\nBody.\n";
        let (meta, body) = extract_frontmatter(input);
        assert!(
            meta.title.is_none(),
            "title should not parse on malformed YAML"
        );
        assert_eq!(
            body, input,
            "malformed-yaml input should pass through unchanged"
        );
    }

    #[test]
    fn extract_frontmatter_empty_block_is_default_metadata() {
        let input = "---\n---\nBody.\n";
        let (meta, body) = extract_frontmatter(input);
        assert!(meta.title.is_none());
        assert!(meta.extras.is_empty());
        // Empty frontmatter block IS stripped — the `---\n---\n` are
        // gone, leaving only the body.
        assert_eq!(body, "Body.\n");
    }
}

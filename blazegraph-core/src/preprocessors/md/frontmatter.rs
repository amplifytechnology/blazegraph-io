//! YAML frontmatter pre-pass for the generic markdown channel.
//!
//! Recognizes the `---\n…\n---\n` block convention at the top of a
//! markdown file. The parsed frontmatter populates a
//! [`crate::types::DocumentMetadata`] via the channel-agnostic
//! [`MetadataExtractor`] trait (CR-57 / v2.1.0 wire-format).
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
//! `serde_json::Value` so the public surface depends only on serde_json.
//! The YAML lib coupling lives entirely inside this module — swapping to
//! a different YAML engine is a one-file change.
//!
//! ## Where the per-field dispatch lives
//!
//! Pre-CR-57: the YAML-to-`DocumentMetadata` dispatch was flat and
//! lived here. Post-CR-57 it lives in [`MdMetadataExtractor`] (in
//! [`crate::preprocessors::md::metadata`]) so the discipline-not-data
//! contract from `09-metadata-first-class.md` § The trait shape is
//! honored — same shape as PDF, DOCX, and any future channel.

use crate::preprocessors::md::metadata::MdMetadataExtractor;
use crate::preprocessors::metadata::extract_document_metadata;
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
/// - `metadata` carries the parsed frontmatter assembled through the
///   [`MdMetadataExtractor`] trait impl (canonical fields flat at the
///   top; strong-convention + opaque keys under `metadata.md`).
///   [`DocumentMetadata::default`] when there is no frontmatter or it
///   fails to parse.
/// - `body` is the input slice with the frontmatter block (and its
///   trailing newline) stripped. The original input slice unchanged
///   when there's no frontmatter.
///
/// Lenient by design: malformed YAML returns the default metadata and
/// the unchanged input, so the `---` block is interpreted as ordinary
/// markdown by the downstream parser.
pub fn extract_frontmatter(input: &str) -> (DocumentMetadata, &str) {
    let Some(closer_offset) = find_frontmatter_block(input) else {
        return (DocumentMetadata::default(), input);
    };

    let matter = Matter::<YAML>::new();
    let Ok(parsed) = matter.parse(input) else {
        // YAML parse failed → lenient: keep the input unchanged so
        // the `---` block is interpreted as markdown content.
        return (DocumentMetadata::default(), input);
    };

    let pod = parsed.data.unwrap_or(Pod::Null);

    let metadata = pod_to_metadata(&pod);

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
fn find_frontmatter_block(input: &str) -> Option<usize> {
    let first_line_end = input.find('\n')?;
    let first_line = &input[..first_line_end];
    if first_line.trim_end() != "---" {
        return None;
    }

    let mut cursor = first_line_end + 1;
    let mut lines_scanned = 0;
    while cursor < input.len() && lines_scanned < FRONTMATTER_CLOSER_SEARCH_LIMIT {
        let rest = &input[cursor..];
        let line_end = rest.find('\n').map(|n| cursor + n).unwrap_or(input.len());
        let line = &input[cursor..line_end];
        if line.trim_end() == "---" {
            return Some(line_end + 1);
        }
        cursor = line_end + 1;
        lines_scanned += 1;
    }
    None
}

/// Project a `gray_matter::Pod` (the lib's untyped YAML representation)
/// onto a `BTreeMap<String, serde_json::Value>` — the format
/// [`MdMetadataExtractor`] consumes. Top-level maps project key by key;
/// anything else (a bare scalar, an array, null) returns an empty map
/// (lenient).
fn pod_to_frontmatter_map(pod: &Pod) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    let Pod::Hash(map) = pod else {
        return out;
    };
    for (key, value) in map {
        if let Some(json_value) = pod_to_json(value) {
            out.insert(key.clone(), json_value);
        }
    }
    out
}

fn pod_to_metadata(pod: &Pod) -> DocumentMetadata {
    let frontmatter = pod_to_frontmatter_map(pod);
    let extractor = MdMetadataExtractor::from_map(frontmatter);
    extract_document_metadata(&extractor, &())
}

/// Convert a `Pod` to `serde_json::Value` for routing into the
/// extractor's frontmatter map. Returns `None` only for shapes that
/// cannot be projected at all (which `Pod` doesn't have today; all
/// variants are mappable).
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
            // Sorted-key order for canonicalization (graph_sha256
            // invariant requires deterministic serialization).
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
        assert!(meta.md.is_none());
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
        // `date` → canonical `created` per design doc § Notes on `created`.
        assert_eq!(meta.created.as_deref(), Some("2026-05-12"));
        assert_eq!(meta.description.as_deref(), Some("Long form"));
        let md_ns = meta.md.expect("md namespace populated");
        assert_eq!(md_ns.draft, Some(true));
        assert_eq!(
            md_ns.tags,
            vec![
                "rust".to_string(),
                "blazegraph".to_string(),
                "b6".to_string()
            ]
        );
        // No unknown keys → md.extras empty.
        assert!(
            md_ns.extras.is_empty(),
            "canonical-only input should produce empty md.extras; got {:?}",
            md_ns.extras
        );
    }

    #[test]
    fn extract_frontmatter_unknown_fields_go_to_md_extras() {
        let input = "---\n\
                     title: Doc\n\
                     custom_key: custom_value\n\
                     priority: 7\n\
                     ---\n\
                     Body.\n";
        let (meta, _body) = extract_frontmatter(input);
        assert_eq!(meta.title.as_deref(), Some("Doc"));
        let md_ns = meta.md.expect("md namespace populated");
        assert_eq!(
            md_ns.extras.get("custom_key"),
            Some(&serde_json::Value::String("custom_value".to_string())),
        );
        assert_eq!(
            md_ns.extras.get("priority"),
            Some(&serde_json::Value::Number(7.into())),
        );
    }

    #[test]
    fn extract_frontmatter_categories_route_to_md_namespace() {
        let input = "---\n\
                     title: Doc\n\
                     categories: [news, updates]\n\
                     ---\n\
                     Body.\n";
        let (meta, _body) = extract_frontmatter(input);
        let md_ns = meta.md.expect("md namespace populated");
        assert_eq!(
            md_ns.categories,
            vec!["news".to_string(), "updates".to_string()]
        );
    }

    #[test]
    fn extract_frontmatter_malformed_yaml_is_lenient() {
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
        // Empty frontmatter block IS stripped — the `---\n---\n` are
        // gone, leaving only the body.
        assert_eq!(body, "Body.\n");
        // Empty hash → empty channel-md namespace (extractor still
        // returns Some(MdMetadata::default()) so downstream code that
        // assumes `md.is_some()` after parsing MD frontmatter holds).
        assert!(
            meta.md.as_ref().map(|m| m.extras.is_empty()).unwrap_or(true)
        );
    }
}

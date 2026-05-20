//! Markdown channel — [`MetadataExtractor`] impl.
//!
//! Replaces the flat dispatch in
//! [`crate::preprocessors::md::frontmatter`] (CR-57 Phase B). The
//! pre-parsed frontmatter map lives on `self`; each trait method is a
//! cheap `BTreeMap` lookup with its source slot declared in its doc
//! comment.
//!
//! Field migration from the pre-CR-57 flat `DocumentMetadata`:
//! - `title` / `author` / `description` / `language` → canonical
//! - `date` → canonical `created` (per `09-metadata-first-class.md` §
//!   Notes on `created`)
//! - `tags` / `draft` → `md.tags` / `md.draft`
//! - `categories` → `md.categories` (newly promoted to strong-convention)
//! - everything else → `md.extras`

use crate::preprocessors::metadata::MetadataExtractor;
use crate::types::{ChannelMetadata, MdMetadata};
use std::collections::BTreeMap;

/// Pre-parsed frontmatter held as a `BTreeMap<String, serde_json::Value>`
/// so the public surface depends only on serde_json (the YAML library
/// coupling stays in [`crate::preprocessors::md::frontmatter`]).
#[derive(Debug, Clone, Default)]
pub struct MdMetadataExtractor {
    frontmatter: BTreeMap<String, serde_json::Value>,
}

impl MdMetadataExtractor {
    /// Construct an extractor from a pre-parsed frontmatter map. Used by
    /// [`crate::preprocessors::md::frontmatter::extract_frontmatter`]
    /// after it has run its YAML pre-pass.
    pub fn from_map(frontmatter: BTreeMap<String, serde_json::Value>) -> Self {
        Self { frontmatter }
    }

    fn get_string(&self, key: &str) -> Option<String> {
        self.frontmatter
            .get(key)
            .and_then(|v| match v {
                serde_json::Value::String(s) => Some(s.clone()),
                // YAML frontmatter may parse `date: 2026-05-12` as an
                // integer-derived value or other scalar; stringify so the
                // lossless free-form representation lands on the canonical
                // `created` slot.
                serde_json::Value::Number(n) => Some(n.to_string()),
                serde_json::Value::Bool(b) => Some(b.to_string()),
                _ => None,
            })
    }
}

impl MetadataExtractor for MdMetadataExtractor {
    type Input = ();

    /// MD title source: frontmatter `title` only. Null when absent.
    /// No body-side fallback — body H1 is composition's job, not
    /// extraction's (see `09-metadata-first-class.md` § F-02).
    fn extract_title(&self, _: &()) -> Option<String> {
        self.get_string("title")
    }

    /// MD author source: frontmatter `author` only. Null when absent.
    fn extract_author(&self, _: &()) -> Option<String> {
        self.get_string("author")
    }

    /// MD description source: frontmatter `description` only.
    fn extract_description(&self, _: &()) -> Option<String> {
        self.get_string("description")
    }

    /// MD language source: frontmatter `language` only. No canonical
    /// frontmatter convention for MD language across the ecosystem; we
    /// accept null when absent.
    fn extract_language(&self, _: &()) -> Option<String> {
        self.get_string("language")
    }

    /// MD created source: frontmatter `date` only. The canonical field
    /// is `created` (per `09-metadata-first-class.md` § Notes on
    /// `created`); the MD ecosystem convention is `date`, so this method
    /// maps it.
    ///
    /// No file-mtime fallback (CR-56 § Invariance; also: MD files sent
    /// over HTTP / scp without `-p` lose meaningful mtime).
    fn extract_created(&self, _: &()) -> Option<String> {
        self.get_string("date")
    }

    /// MD channel-specific bag. Strong-convention flat fields (`draft`,
    /// `tags`, `categories`); everything else (Hugo `slug`, Astro
    /// `pubDate`, Obsidian aliases, etc.) lands in `md.extras` keyed by
    /// raw frontmatter key.
    fn extract_channel_metadata(&self, _: &()) -> ChannelMetadata {
        const CANONICAL_KEYS: &[&str] =
            &["title", "author", "description", "language", "date"];
        const STRONG_CONVENTION_KEYS: &[&str] = &["draft", "tags", "categories"];

        let mut md = MdMetadata {
            draft: self.frontmatter.get("draft").and_then(|v| v.as_bool()),
            tags: self
                .frontmatter
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            categories: self
                .frontmatter
                .get("categories")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            extras: BTreeMap::new(),
        };

        for (key, value) in &self.frontmatter {
            if CANONICAL_KEYS.contains(&key.as_str())
                || STRONG_CONVENTION_KEYS.contains(&key.as_str())
            {
                continue;
            }
            md.extras.insert(key.clone(), value.clone());
        }

        ChannelMetadata::Md(md)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preprocessors::metadata::extract_document_metadata;

    fn fm(pairs: &[(&str, serde_json::Value)]) -> BTreeMap<String, serde_json::Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn canonical_fields_route_to_top_level() {
        let map = fm(&[
            ("title", serde_json::json!("Hello")),
            ("author", serde_json::json!("Alice")),
            ("description", serde_json::json!("Abstract")),
            ("language", serde_json::json!("en")),
            ("date", serde_json::json!("2026-05-20")),
        ]);
        let extractor = MdMetadataExtractor::from_map(map);
        let md = extract_document_metadata(&extractor, &());
        assert_eq!(md.title.as_deref(), Some("Hello"));
        assert_eq!(md.author.as_deref(), Some("Alice"));
        assert_eq!(md.description.as_deref(), Some("Abstract"));
        assert_eq!(md.language.as_deref(), Some("en"));
        // `date` arrives on canonical `created`.
        assert_eq!(md.created.as_deref(), Some("2026-05-20"));
    }

    #[test]
    fn strong_convention_fields_route_to_md_namespace() {
        let map = fm(&[
            ("draft", serde_json::json!(true)),
            ("tags", serde_json::json!(["rust", "blazegraph"])),
            ("categories", serde_json::json!(["news"])),
        ]);
        let extractor = MdMetadataExtractor::from_map(map);
        let md = extract_document_metadata(&extractor, &());
        let md_ns = md.md.expect("md namespace populated");
        assert_eq!(md_ns.draft, Some(true));
        assert_eq!(md_ns.tags, vec!["rust".to_string(), "blazegraph".to_string()]);
        assert_eq!(md_ns.categories, vec!["news".to_string()]);
        assert!(md_ns.extras.is_empty());
    }

    #[test]
    fn unknown_keys_route_to_md_extras() {
        let map = fm(&[
            ("title", serde_json::json!("Doc")),
            ("layout", serde_json::json!("post")),
            ("priority", serde_json::json!(7)),
        ]);
        let extractor = MdMetadataExtractor::from_map(map);
        let md = extract_document_metadata(&extractor, &());
        let md_ns = md.md.expect("md namespace populated");
        assert_eq!(md_ns.extras.len(), 2);
        assert_eq!(
            md_ns.extras.get("layout"),
            Some(&serde_json::Value::String("post".to_string())),
        );
        assert_eq!(
            md_ns.extras.get("priority"),
            Some(&serde_json::Value::Number(7.into())),
        );
    }

    #[test]
    fn empty_frontmatter_produces_all_none_plus_empty_namespace() {
        let extractor = MdMetadataExtractor::default();
        let md = extract_document_metadata(&extractor, &());
        assert!(md.title.is_none());
        assert!(md.created.is_none());
        let md_ns = md.md.expect("md namespace populated even when empty");
        assert!(md_ns.draft.is_none());
        assert!(md_ns.tags.is_empty());
        assert!(md_ns.extras.is_empty());
    }
}

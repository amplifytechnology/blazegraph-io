//! PDF channel — [`MetadataExtractor`] impl.
//!
//! Replaces the flat `extract_enhanced_metadata` in
//! [`crate::preprocessors::pdf::xhtml_parser`] (CR-57 Phase B). The
//! pre-parsed XMP / `<meta>` tag map lives on `self`; each trait method
//! is a cheap HashMap lookup with its source slot declared in its doc
//! comment.
//!
//! **Closed asymmetry:** the `_ => {}` drop site at the prior flat
//! extractor's switch is replaced by a tag-passthrough into `pdf.extras`
//! — any XMP / `<meta>` tag that is neither canonical (handled by trait
//! methods) nor a strong-convention PDF field surfaces in `extras` keyed
//! by its raw tag name. Per `09-metadata-first-class.md` § Channel-specific
//! (pdf), this closes a long-standing PDF-vs-MD passthrough imbalance.

use crate::preprocessors::metadata::MetadataExtractor;
use crate::types::{ChannelMetadata, PdfMetadata};
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

static META_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<meta\s+name="([^"]*)"[^>]*content="([^"]*)"[^>]*/?>"#).unwrap()
});

/// Pre-parses Tika XHTML's `<meta>` tags once at construction; each trait
/// method is a cheap `HashMap` lookup. Lifetime tied to the source XHTML
/// to avoid copies during construction — the extractor borrows the source
/// string for the duration of one extraction call.
pub struct PdfMetadataExtractor<'a> {
    meta_tags: HashMap<&'a str, &'a str>,
}

impl<'a> PdfMetadataExtractor<'a> {
    /// Parse `<meta name="…" content="…"/>` tags from the XHTML into a
    /// lookup map. Tags repeated under the same name keep the last value
    /// seen (Tika emits each canonical tag once in practice, but the
    /// "last wins" rule is defensive against producer quirks).
    pub fn new(xhtml: &'a str) -> Self {
        let mut meta_tags = HashMap::new();
        for cap in META_REGEX.captures_iter(xhtml) {
            if let (Some(name), Some(content)) = (cap.get(1), cap.get(2)) {
                meta_tags.insert(name.as_str(), content.as_str());
            }
        }
        Self { meta_tags }
    }
}

impl<'a> MetadataExtractor for PdfMetadataExtractor<'a> {
    type Input = (); // pre-parsed state lives on `self`

    /// PDF title source: `dc:title` only. Null when absent.
    /// Title cleanup (e.g., dvi-filename leakage in `shannon1948.dvi`) is a
    /// composition-layer concern handled by URD or downstream — not
    /// extraction. See `09-metadata-first-class.md` § F-02 — deferred to
    /// composition layer.
    fn extract_title(&self, _: &()) -> Option<String> {
        self.meta_tags.get("dc:title").map(|s| (*s).to_string())
    }

    /// PDF author source: `dc:creator` only. Null when absent.
    fn extract_author(&self, _: &()) -> Option<String> {
        self.meta_tags.get("dc:creator").map(|s| (*s).to_string())
    }

    /// PDF description source: `dc:description` only. Null when absent.
    fn extract_description(&self, _: &()) -> Option<String> {
        self.meta_tags
            .get("dc:description")
            .map(|s| (*s).to_string())
    }

    /// PDF language source: `dc:language` only. Null when absent.
    fn extract_language(&self, _: &()) -> Option<String> {
        self.meta_tags.get("dc:language").map(|s| (*s).to_string())
    }

    /// PDF created source: `dcterms:created` only. Null when absent.
    /// No file-mtime fallback (CR-56 § Invariance).
    fn extract_created(&self, _: &()) -> Option<String> {
        self.meta_tags
            .get("dcterms:created")
            .map(|s| (*s).to_string())
    }

    /// PDF channel-specific bag. Strong-convention typed fields + extras
    /// passthrough. The extras passthrough closes the prior asymmetry
    /// where unrecognized XMP tags were silently dropped.
    fn extract_channel_metadata(&self, _: &()) -> ChannelMetadata {
        const CANONICAL_TAGS: &[&str] = &[
            "dc:title",
            "dc:creator",
            "dc:language",
            "dc:description",
            "dcterms:created",
        ];
        const STRONG_CONVENTION_TAGS: &[&str] = &[
            "xmp:dc:publisher",
            "dc:publisher",
            "xmp:CreatorTool",
            "pdf:producer",
            "pdf:PDFVersion",
            "dcterms:modified",
            "pdf:encrypted",
            "pdf:hasMarkedContent",
            "xmpTPg:NPages",
        ];

        let mut pdf = PdfMetadata {
            publisher: self
                .meta_tags
                .get("xmp:dc:publisher")
                .or_else(|| self.meta_tags.get("dc:publisher"))
                .map(|s| (*s).to_string()),
            creator_tool: self
                .meta_tags
                .get("xmp:CreatorTool")
                .map(|s| (*s).to_string()),
            producer: self.meta_tags.get("pdf:producer").map(|s| (*s).to_string()),
            version: self
                .meta_tags
                .get("pdf:PDFVersion")
                .map(|s| (*s).to_string()),
            modified: self
                .meta_tags
                .get("dcterms:modified")
                .map(|s| (*s).to_string()),
            encrypted: self.meta_tags.get("pdf:encrypted").map(|s| *s == "true"),
            has_marked_content: self
                .meta_tags
                .get("pdf:hasMarkedContent")
                .map(|s| *s == "true"),
            page_count: self
                .meta_tags
                .get("xmpTPg:NPages")
                .and_then(|s| s.parse().ok()),
            extras: BTreeMap::new(),
        };

        // Asymmetry close: any tag that is neither canonical (handled by
        // trait methods above) nor a strong-convention pdf-namespace field
        // gets surfaced in pdf.extras with its raw name as the key.
        // BTreeMap iteration is ordered, so canonicalization is
        // deterministic — same input PDF always produces the same
        // graph_sha256.
        for (name, content) in &self.meta_tags {
            if CANONICAL_TAGS.contains(name) || STRONG_CONVENTION_TAGS.contains(name) {
                continue;
            }
            pdf.extras.insert(
                (*name).to_string(),
                serde_json::Value::String((*content).to_string()),
            );
        }

        ChannelMetadata::Pdf(pdf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preprocessors::metadata::extract_document_metadata;

    fn xhtml_with(meta: &[(&str, &str)]) -> String {
        let mut s = String::from("<html><head>");
        for (name, content) in meta {
            s.push_str(&format!("<meta name=\"{name}\" content=\"{content}\"/>"));
        }
        s.push_str("</head><body/></html>");
        s
    }

    #[test]
    fn canonical_fields_route_to_top_level() {
        let xhtml = xhtml_with(&[
            ("dc:title", "Hello"),
            ("dc:creator", "Alice"),
            ("dc:language", "en"),
            ("dc:description", "An abstract"),
            ("dcterms:created", "2026-05-20"),
        ]);
        let extractor = PdfMetadataExtractor::new(&xhtml);
        let md = extract_document_metadata(&extractor, &());
        assert_eq!(md.title.as_deref(), Some("Hello"));
        assert_eq!(md.author.as_deref(), Some("Alice"));
        assert_eq!(md.language.as_deref(), Some("en"));
        assert_eq!(md.description.as_deref(), Some("An abstract"));
        assert_eq!(md.created.as_deref(), Some("2026-05-20"));
    }

    #[test]
    fn pdf_strong_convention_fields_route_to_pdf_namespace() {
        let xhtml = xhtml_with(&[
            ("pdf:producer", "Acrobat"),
            ("pdf:PDFVersion", "1.4"),
            ("xmp:CreatorTool", "TeX"),
            ("xmp:dc:publisher", "ACM"),
            ("xmpTPg:NPages", "42"),
            ("pdf:encrypted", "false"),
            ("pdf:hasMarkedContent", "true"),
            ("dcterms:modified", "2026-05-20T01:02:03Z"),
        ]);
        let extractor = PdfMetadataExtractor::new(&xhtml);
        let md = extract_document_metadata(&extractor, &());
        let pdf = md.pdf.expect("pdf namespace populated");
        assert_eq!(pdf.producer.as_deref(), Some("Acrobat"));
        assert_eq!(pdf.version.as_deref(), Some("1.4"));
        assert_eq!(pdf.creator_tool.as_deref(), Some("TeX"));
        assert_eq!(pdf.publisher.as_deref(), Some("ACM"));
        assert_eq!(pdf.page_count, Some(42));
        assert_eq!(pdf.encrypted, Some(false));
        assert_eq!(pdf.has_marked_content, Some(true));
        assert_eq!(pdf.modified.as_deref(), Some("2026-05-20T01:02:03Z"));
        assert!(pdf.extras.is_empty());
    }

    #[test]
    fn non_canonical_tags_route_to_pdf_extras() {
        let xhtml = xhtml_with(&[
            ("dc:title", "T"),
            ("pdf:producer", "P"),
            ("xmp:custom_field", "abc"),
            ("custom:WeirdTag", "xyz"),
        ]);
        let extractor = PdfMetadataExtractor::new(&xhtml);
        let md = extract_document_metadata(&extractor, &());
        let pdf = md.pdf.expect("pdf namespace populated");
        assert_eq!(pdf.extras.len(), 2);
        assert_eq!(
            pdf.extras.get("xmp:custom_field"),
            Some(&serde_json::Value::String("abc".to_string())),
        );
        assert_eq!(
            pdf.extras.get("custom:WeirdTag"),
            Some(&serde_json::Value::String("xyz".to_string())),
        );
    }

    #[test]
    fn publisher_falls_back_from_xmp_to_dc() {
        let xhtml = xhtml_with(&[("dc:publisher", "fallback ACM")]);
        let extractor = PdfMetadataExtractor::new(&xhtml);
        let md = extract_document_metadata(&extractor, &());
        let pdf = md.pdf.expect("pdf namespace populated");
        assert_eq!(pdf.publisher.as_deref(), Some("fallback ACM"));
    }

    #[test]
    fn empty_xhtml_produces_all_none_plus_empty_namespace() {
        let extractor = PdfMetadataExtractor::new("<html/>");
        let md = extract_document_metadata(&extractor, &());
        assert!(md.title.is_none());
        assert!(md.author.is_none());
        let pdf = md.pdf.expect("pdf namespace populated");
        assert!(pdf.producer.is_none());
        assert!(pdf.extras.is_empty());
    }
}

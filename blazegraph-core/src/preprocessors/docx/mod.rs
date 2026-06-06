//! DOCX preprocessor.
//!
//! ## Body channel (C1, S10)
//!
//! [`body::parse_docx`] is the live entry point: `.docx` zip bytes →
//! `DocumentGraph`, mirroring the markdown channel's
//! [`crate::preprocessors::md::generic_md::parse`]. It owns the OOXML
//! container read, the `styles.xml` resolution map, the `<w:body>` walk, and
//! the Section/Paragraph/Table/Blockquote projection with emphasis
//! canonicalization.
//!
//! ## Refs channel (C2, S10)
//!
//! [`rels`] resolves `<w:hyperlink r:id>` → URL + `TargetMode` via
//! `word/_rels/document.xml.rels`, feeding the body walk's ref attribution.
//!
//! ## Metadata channel (C3, S10)
//!
//! [`props`] reads `docProps/core.xml` + `docProps/app.xml` into flat
//! `local_name → text` maps; [`DocxMetadataExtractor`] holds those maps and
//! implements [`MetadataExtractor`], populating the canonical fields
//! (`title`/`author`/`description`/`language`/`created`) from Dublin Core in
//! `core.xml` and the `docx:` namespace ([`DocxMetadata`]) from `app.xml` +
//! the `core.xml` leftovers. [`body::parse_docx`] wires it: read the docProps
//! parts, build the extractor, call
//! [`crate::preprocessors::metadata::extract_document_metadata`], assign to
//! `graph.document_info.document_metadata`.

pub mod body;
pub(crate) mod props;
pub(crate) mod rels;

pub use body::parse_docx;

use crate::preprocessors::metadata::MetadataExtractor;
use crate::types::{ChannelMetadata, DocxMetadata};
use props::DocxProps;

/// DOCX metadata extractor. Wraps the parsed `docProps` maps
/// ([`DocxProps`]) and implements [`MetadataExtractor`] over them.
///
/// Per the contract (C0 design notes, decision 8) + the two-tier model
/// (`09-metadata-first-class.md`): canonical Dublin-Core fields come from
/// `docProps/core.xml`; the `docx:` namespace comes from `docProps/app.xml`
/// plus the non-canonical `core.xml` leftovers (`cp:lastModifiedBy`,
/// `cp:revision`, `dcterms:modified`).
///
/// **No mtime/body/locale fallbacks** — an absent source slot yields `None`
/// (CR-56 § Invariance / § I.3). The pre-parsed maps live on `self`, so
/// `type Input = ()` (the documented pattern).
#[derive(Debug, Default, Clone)]
pub struct DocxMetadataExtractor {
    props: DocxProps,
}

impl DocxMetadataExtractor {
    /// Build an extractor over already-parsed docProps. Callers parse the two
    /// XML parts once (`DocxProps::parse`) and hand the result in.
    pub(super) fn new(props: DocxProps) -> Self {
        Self { props }
    }
}

impl MetadataExtractor for DocxMetadataExtractor {
    type Input = ();

    /// Title source: `core.xml` `dc:title`. No body fallback (CR-56 § I.3 —
    /// a heading paragraph is body content, not metadata). Absent → `None`.
    fn extract_title(&self, _: &()) -> Option<String> {
        self.props.core("title").map(str::to_string)
    }

    /// Author source: `core.xml` `dc:creator`. Absent → `None`.
    fn extract_author(&self, _: &()) -> Option<String> {
        self.props.core("creator").map(str::to_string)
    }

    /// Description source: `core.xml` `dc:description`. Absent → `None`.
    fn extract_description(&self, _: &()) -> Option<String> {
        self.props.core("description").map(str::to_string)
    }

    /// Language source: `core.xml` `dc:language` only. **No mtime/locale
    /// fallback** — `dc:language` is frequently absent (neither fixture has
    /// it), in which case this returns `None`.
    fn extract_language(&self, _: &()) -> Option<String> {
        self.props.core("language").map(str::to_string)
    }

    /// Created source: `core.xml` `dcterms:created` (W3CDTF / ISO-8601).
    /// **No file-mtime fallback** (CR-56 § Invariance — an mtime fallback
    /// breaks bgraph.md ↔ bgraph.json canonical invariance). Absent → `None`.
    fn extract_created(&self, _: &()) -> Option<String> {
        self.props.core("created").map(str::to_string)
    }

    /// `docx:` namespace bag ([`DocxMetadata`]). Strong-convention typed
    /// fields drawn from two source slots:
    /// - **`app.xml`** (extended-properties): `Application`, `AppVersion`,
    ///   `Pages`, `Words`, `Characters`, `Lines`, `Paragraphs`, `Company`,
    ///   `Manager`, `Template`, `TotalTime`, `DocSecurity`. Numeric fields
    ///   parse leniently (absent / non-numeric → `None`, never an error).
    /// - **`core.xml` leftovers** (non-canonical): `cp:lastModifiedBy`,
    ///   `cp:revision`, `dcterms:modified`.
    ///
    /// Any *other* present `core.xml` leaf that isn't a canonical field (e.g.
    /// `keywords`, `subject`, `category`) surfaces in `extras` keyed by its
    /// local name, so nothing is silently dropped. `extras` is a `BTreeMap`,
    /// so serialization stays deterministic (cache-stable `graph_sha256`).
    /// Absent fields → `None` / omitted.
    fn extract_channel_metadata(&self, _: &()) -> ChannelMetadata {
        // Canonical core.xml fields are surfaced by the trait methods above —
        // exclude them from the `extras` passthrough. The non-canonical
        // leftovers promoted to typed `docx:` fields are likewise excluded.
        const CANONICAL_CORE: &[&str] = &["title", "creator", "description", "language", "created"];
        const PROMOTED_CORE: &[&str] = &["lastModifiedBy", "revision", "modified"];

        let p = &self.props;
        let mut docx = DocxMetadata {
            // app.xml — strings.
            application: p.app("Application").map(str::to_string),
            app_version: p.app("AppVersion").map(str::to_string),
            company: p.app("Company").map(str::to_string),
            manager: p.app("Manager").map(str::to_string),
            template: p.app("Template").map(str::to_string),
            // app.xml — lenient numerics (absent / non-numeric → None).
            pages: p.app_u32("Pages"),
            words: p.app_u32("Words"),
            characters: p.app_u32("Characters"),
            lines: p.app_u32("Lines"),
            paragraphs: p.app_u32("Paragraphs"),
            total_time: p.app_u32("TotalTime"),
            doc_security: p.app_u32("DocSecurity"),
            // core.xml leftovers.
            last_modified_by: p.core("lastModifiedBy").map(str::to_string),
            revision: p.core("revision").map(str::to_string),
            modified: p.core("modified").map(str::to_string),
            extras: std::collections::BTreeMap::new(),
        };

        // Passthrough: any other present core.xml leaf (not canonical, not a
        // promoted typed field) → extras, keyed by local name. Deterministic
        // via the BTreeMap. (app.xml leftovers — Template etc. — are all
        // already promoted to typed fields, so only core.xml needs sweeping.)
        for (name, value) in &p.core {
            if CANONICAL_CORE.contains(&name.as_str()) || PROMOTED_CORE.contains(&name.as_str()) {
                continue;
            }
            docx.extras
                .insert(name.clone(), serde_json::Value::String(value.clone()));
        }

        ChannelMetadata::Docx(docx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preprocessors::metadata::extract_document_metadata;

    /// Build an extractor straight from the on-disk fixture's docProps parts,
    /// mirroring what `parse_docx` does internally — so the test asserts the
    /// real OOXML values, not synthetic XML.
    fn extractor_for(fixture: &str) -> DocxMetadataExtractor {
        use std::io::Read;
        use std::path::PathBuf;

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test_fixtures/docx")
            .join(fixture);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {path:?}: {e}"));
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();

        let read = |archive: &mut zip::ZipArchive<_>, name: &str| -> String {
            let mut buf = String::new();
            if let Ok(mut f) = archive.by_name(name) {
                f.read_to_string(&mut buf).ok();
            }
            buf
        };
        let core = read(&mut archive, "docProps/core.xml");
        let app = read(&mut archive, "docProps/app.xml");
        DocxMetadataExtractor::new(DocxProps::parse(&core, &app).unwrap())
    }

    #[test]
    fn metadata_extracted() {
        // Real extraction from `structured.docx`'s docProps (replaces the C1
        // stub test). Canonical Dublin-Core fields + the populated `docx:`
        // namespace, asserted against the fixture's actual values.
        let md = extract_document_metadata(&extractor_for("structured.docx"), &());

        // Canonical core.xml fields.
        assert_eq!(md.title.as_deref(), Some("Structured Sample Document"));
        assert_eq!(md.author.as_deref(), Some("John Smith"));
        assert_eq!(md.description.as_deref(), Some("generated by python-docx"));
        assert_eq!(md.created.as_deref(), Some("2026-01-01T00:00:00Z"));
        // dc:language is absent in the fixture → None (no locale fallback).
        assert!(md.language.is_none(), "no dc:language → None");

        // Cross-channel slots stay empty.
        assert!(md.pdf.is_none());
        assert!(md.md.is_none());

        // docx: namespace from app.xml + core.xml leftovers.
        let docx = md.docx.expect("docx namespace populated");
        assert_eq!(
            docx.application.as_deref(),
            Some("Microsoft Macintosh Word")
        );
        assert_eq!(docx.app_version.as_deref(), Some("14.0000"));
        assert_eq!(docx.template.as_deref(), Some("Normal.dotm"));
        assert_eq!(docx.pages, Some(1));
        // Words/Characters/Lines/Paragraphs are present-but-zero in the
        // python-docx template — parsed, not dropped.
        assert_eq!(docx.words, Some(0));
        assert_eq!(docx.total_time, Some(0));
        assert_eq!(docx.doc_security, Some(0));
        // Empty placeholders (`<Manager/>`, `<Company/>`) → None.
        assert!(docx.company.is_none(), "empty <Company/> → None");
        assert!(docx.manager.is_none(), "empty <Manager/> → None");
        // core.xml leftovers promoted to typed docx: fields.
        assert_eq!(docx.last_modified_by.as_deref(), Some("John Smith"));
        assert_eq!(docx.revision.as_deref(), Some("1"));
        assert_eq!(docx.modified.as_deref(), Some("2026-01-01T00:00:00Z"));
        // No non-canonical core leftovers remain in this fixture (subject /
        // keywords / category are empty placeholders, dropped by the reader).
        assert!(
            docx.extras.is_empty(),
            "no stray core.xml leftovers in extras; got {:?}",
            docx.extras
        );
    }

    #[test]
    fn with_links_metadata_extracted() {
        // The second fixture: same author, different title — confirms the
        // extractor reads each document's own docProps, not a cached value.
        let md = extract_document_metadata(&extractor_for("with_links.docx"), &());
        assert_eq!(md.title.as_deref(), Some("Hyperlink Sample Document"));
        assert_eq!(md.author.as_deref(), Some("John Smith"));
        assert!(md.language.is_none());
    }

    #[test]
    fn empty_props_yield_empty_namespace() {
        // Defensive: an extractor over empty docProps (the package had no
        // core/app parts) produces all-None canonical fields + a default,
        // empty docx: namespace — never an error, never a panic.
        let md = extract_document_metadata(&DocxMetadataExtractor::new(DocxProps::default()), &());
        assert!(md.title.is_none());
        assert!(md.author.is_none());
        assert!(md.created.is_none());
        let docx = md.docx.expect("docx namespace always present");
        assert!(docx.application.is_none());
        assert!(docx.extras.is_empty());
    }
}

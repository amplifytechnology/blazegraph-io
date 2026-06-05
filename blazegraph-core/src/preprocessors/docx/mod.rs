//! DOCX preprocessor — stub.
//!
//! The DOCX channel is **not yet implemented** (Track C is S10). This
//! module exists so the schema's [`DocxMetadata`] type + the
//! [`ChannelMetadata::Docx`] variant have a corresponding extractor
//! implementation, keeping the type-system surface complete from day one
//! of v2.1.0.
//!
//! When S10 lands the real DOCX channel, [`DocxMetadataExtractor`] gets
//! its method bodies populated from `docProps/core.xml`,
//! `docProps/app.xml`, and `docProps/custom.xml` per the inventory probe
//! at `scripts/docx_metadata_probe.py`.
//!
//! ## Body channel (C1, S10)
//!
//! [`body::parse_docx`] is the live entry point: `.docx` zip bytes →
//! `DocumentGraph`, mirroring the markdown channel's
//! [`crate::preprocessors::md::generic_md::parse`]. It owns the OOXML
//! container read, the `styles.xml` resolution map, the `<w:body>` walk, and
//! the Section/Paragraph/Table/Blockquote projection with emphasis
//! canonicalization. Metadata (`DocxMetadataExtractor`) and ref extraction
//! land in later handoffs (C3 / C2).

pub mod body;

pub use body::parse_docx;

use crate::preprocessors::metadata::MetadataExtractor;
use crate::types::{ChannelMetadata, DocxMetadata};

/// Stub extractor for the DOCX channel. All canonical methods return
/// `None`; the channel bag returns a default [`DocxMetadata`].
///
/// TODO(S10): wire to the OOXML probe; doc-comment each canonical field
/// with the `docProps/*.xml` source slot it reads.
#[derive(Debug, Default, Clone, Copy)]
pub struct DocxMetadataExtractor;

impl DocxMetadataExtractor {
    pub fn new() -> Self {
        Self
    }
}

impl MetadataExtractor for DocxMetadataExtractor {
    type Input = ();

    /// TODO(S10): `core:title`. Stub returns `None`.
    fn extract_title(&self, _: &()) -> Option<String> {
        None
    }

    /// TODO(S10): `core:creator`. Stub returns `None`.
    fn extract_author(&self, _: &()) -> Option<String> {
        None
    }

    /// TODO(S10): `core:description`. Stub returns `None`.
    fn extract_description(&self, _: &()) -> Option<String> {
        None
    }

    /// TODO(S10): `core:language`. Stub returns `None`.
    fn extract_language(&self, _: &()) -> Option<String> {
        None
    }

    /// TODO(S10): `core:created`. Stub returns `None`.
    /// No file-mtime fallback (CR-56 § Invariance).
    fn extract_created(&self, _: &()) -> Option<String> {
        None
    }

    /// TODO(S10): drive from `docProps/app.xml` + `docProps/custom.xml`.
    /// Stub returns a default [`DocxMetadata`].
    fn extract_channel_metadata(&self, _: &()) -> ChannelMetadata {
        ChannelMetadata::Docx(DocxMetadata::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preprocessors::metadata::extract_document_metadata;

    #[test]
    fn stub_extractor_produces_empty_docx_namespace() {
        let md = extract_document_metadata(&DocxMetadataExtractor::new(), &());
        assert!(md.title.is_none());
        assert!(md.author.is_none());
        assert!(md.description.is_none());
        assert!(md.language.is_none());
        assert!(md.created.is_none());
        assert!(md.pdf.is_none());
        assert!(md.md.is_none());
        let docx = md.docx.expect("docx namespace populated");
        assert!(docx.application.is_none());
        assert!(docx.extras.is_empty());
    }
}

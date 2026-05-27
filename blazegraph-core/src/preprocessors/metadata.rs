//! Channel-agnostic metadata extraction trait + cross-channel assembly.
//!
//! The trait's job is **forcing the same fallback-discipline question
//! across every channel**. One method per canonical field; each method's
//! signature forces the implementor to answer "where does this come from
//! in this source, and what's the fallback when it's absent?" The trait
//! isn't about the Venn diagram of data — it's about the *shape of the
//! question* every channel must answer for every canonical field.
//!
//! Design rationale: `docs/P2/core/architecture/09-metadata-first-class.md`
//! § The trait shape — discipline before data. Wire-format contract:
//! `08-bgraph-md-format.md` § Amendment I.
//!
//! Adding a new canonical field = adding a method here = every existing
//! channel sees the compiler force them to implement it (or explicitly
//! opt out with `None`).
//!
//! Adding a new channel (HTML, EPUB, future formats) = implementing
//! this trait = answering the fallback question for every existing
//! canonical field. The code paths may look drastically different
//! (XHTML walk vs. YAML parse vs. OOXML zip), but the contract is
//! the same.
//!
//! **REQUIRED:** Each trait-method implementation's doc comment declares
//! its fallback chain in this channel. Reviewer-enforced; no compile-time
//! check, but the convention is load-bearing for cross-channel consistency.

use crate::types::{ChannelMetadata, DocumentMetadata};

/// Channel-agnostic metadata extraction contract.
///
/// See the module docstring for the discipline-not-data rationale.
pub trait MetadataExtractor {
    /// The channel's input type — whatever the channel needs to extract
    /// from (parsed XHTML, YAML frontmatter, OOXML doc, etc.). Channels
    /// whose pre-parsed state lives on `self` set this to `()` and ignore
    /// the parameter.
    type Input;

    /// Title source. Doc comment in each impl declares the source slot and
    /// the fallback chain. **No body-side fallback** — body content is
    /// composition's job, not extraction's (CR-56 § I.3 / F-02 deferred).
    fn extract_title(&self, input: &Self::Input) -> Option<String>;

    /// Author source. Doc comment in each impl declares the source slot
    /// and the fallback chain.
    fn extract_author(&self, input: &Self::Input) -> Option<String>;

    /// Description / abstract source. Doc comment declares the source slot.
    fn extract_description(&self, input: &Self::Input) -> Option<String>;

    /// Language source. BCP-47-ish. Doc comment declares the source slot.
    fn extract_language(&self, input: &Self::Input) -> Option<String>;

    /// Created date source. ISO-8601-formatted (where the source provides
    /// a normalized representation).
    ///
    /// **CRITICAL:** no file-mtime fallback — breaks bgraph.md ↔ bgraph.json
    /// canonical invariance (CR-56 § Invariance).
    fn extract_created(&self, input: &Self::Input) -> Option<String>;

    /// Channel-specific namespaced metadata bag. The variant of
    /// [`ChannelMetadata`] returned identifies which namespace populates
    /// on the assembled [`DocumentMetadata`].
    fn extract_channel_metadata(&self, input: &Self::Input) -> ChannelMetadata;
}

/// Assemble a [`DocumentMetadata`] from any channel implementing
/// [`MetadataExtractor`]. Centralized so canonical assembly stays
/// consistent — no channel can accidentally skip a canonical field.
///
/// Adding a new canonical field = adding a method to the trait = adding a
/// line here. The compiler enforces that every existing channel gets
/// updated.
pub fn extract_document_metadata<E: MetadataExtractor>(
    extractor: &E,
    input: &E::Input,
) -> DocumentMetadata {
    let mut md = DocumentMetadata {
        title: extractor.extract_title(input),
        author: extractor.extract_author(input),
        description: extractor.extract_description(input),
        language: extractor.extract_language(input),
        created: extractor.extract_created(input),
        pdf: None,
        md: None,
        docx: None,
    };
    match extractor.extract_channel_metadata(input) {
        ChannelMetadata::Pdf(p) => md.pdf = Some(p),
        ChannelMetadata::Md(m) => md.md = Some(m),
        ChannelMetadata::Docx(d) => md.docx = Some(d),
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChannelMetadata, MdMetadata};

    /// Minimal extractor for unit-testing assembly. Returns canned values
    /// for every method so the assembly contract is exercised end-to-end.
    struct CannedExtractor;

    impl MetadataExtractor for CannedExtractor {
        type Input = ();

        fn extract_title(&self, _: &()) -> Option<String> {
            Some("T".to_string())
        }
        fn extract_author(&self, _: &()) -> Option<String> {
            Some("A".to_string())
        }
        fn extract_description(&self, _: &()) -> Option<String> {
            None
        }
        fn extract_language(&self, _: &()) -> Option<String> {
            Some("en".to_string())
        }
        fn extract_created(&self, _: &()) -> Option<String> {
            Some("2026-05-20".to_string())
        }
        fn extract_channel_metadata(&self, _: &()) -> ChannelMetadata {
            ChannelMetadata::Md(MdMetadata {
                draft: Some(true),
                ..Default::default()
            })
        }
    }

    #[test]
    fn extract_document_metadata_assembles_canonical_plus_namespace() {
        let md = extract_document_metadata(&CannedExtractor, &());
        assert_eq!(md.title.as_deref(), Some("T"));
        assert_eq!(md.author.as_deref(), Some("A"));
        assert!(md.description.is_none());
        assert_eq!(md.language.as_deref(), Some("en"));
        assert_eq!(md.created.as_deref(), Some("2026-05-20"));
        let md_ns = md.md.expect("md namespace populated");
        assert_eq!(md_ns.draft, Some(true));
        assert!(md.pdf.is_none());
        assert!(md.docx.is_none());
    }
}

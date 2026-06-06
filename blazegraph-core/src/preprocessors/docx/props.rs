//! DOCX document-properties reader — `docProps/core.xml` + `docProps/app.xml`.
//!
//! The two docProps parts carry the document's metadata. Unlike the body
//! (`word/document.xml`) and the relationships part (`document.xml.rels`,
//! whose data lives in *attributes*), the docProps values live in **element
//! text content**:
//!
//! ```xml
//! <!-- docProps/core.xml -->
//! <cp:coreProperties xmlns:dc="…" xmlns:cp="…" xmlns:dcterms="…">
//!   <dc:title>Structured Sample Document</dc:title>
//!   <dc:creator>John Smith</dc:creator>
//!   <dcterms:created>2026-01-01T00:00:00Z</dcterms:created>
//! </cp:coreProperties>
//!
//! <!-- docProps/app.xml -->
//! <Properties xmlns="…/extended-properties">
//!   <Application>Microsoft Macintosh Word</Application>
//!   <Pages>1</Pages>
//!   <AppVersion>14.0000</AppVersion>
//! </Properties>
//! ```
//!
//! This reader walks both parts with `quick-xml`, keying each leaf element by
//! its **local name** (namespace-prefix-stripped — `dc:title` → `title`,
//! `cp:lastModifiedBy` → `lastModifiedBy`) and collecting its trimmed text
//! into a flat `local_name → value` map per part. The
//! [`DocxMetadataExtractor`](super::DocxMetadataExtractor) then reads those
//! maps to populate the canonical fields + the `docx:` namespace.
//!
//! **Empty elements are dropped.** python-docx (and Word) emit placeholder
//! empties like `<dc:subject/>`, `<cp:keywords/>`, `<Manager/>`, `<Company/>`
//! — these carry no value, so an element with empty/whitespace-only text is
//! treated as absent (not stored), keeping every downstream lookup honest:
//! "present in the map" ⟺ "had a real value".
//!
//! A missing part yields an empty map (callers pass [`Default::default`] /
//! `""`); malformed XML is a hard [`ParseError::MalformedDocx`], consistent
//! with the rest of the channel (`body.rs` / `rels.rs`).

use std::collections::HashMap;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::super::md::types::ParseError;

/// Parsed `docProps/core.xml` + `docProps/app.xml`, each flattened to a
/// `local_name → text` map. Owned by the
/// [`DocxMetadataExtractor`](super::DocxMetadataExtractor) (`type Input = ()`
/// — the documented "pre-parsed state lives on `self`" pattern).
///
/// Lookups are by **local name** (prefix already stripped): `core` holds
/// `title`, `creator`, `description`, `language`, `created`, `modified`,
/// `lastModifiedBy`, `revision`, `keywords`, … ; `app` holds `Application`,
/// `AppVersion`, `Pages`, `Words`, `Characters`, `Lines`, `Paragraphs`,
/// `Company`, `Manager`, `Template`, `TotalTime`, `DocSecurity`, … .
#[derive(Debug, Default, Clone)]
pub(crate) struct DocxProps {
    /// `docProps/core.xml` — Dublin Core + core-properties leaves.
    pub(super) core: HashMap<String, String>,
    /// `docProps/app.xml` — extended (application) properties leaves.
    pub(super) app: HashMap<String, String>,
}

impl DocxProps {
    /// Parse the two docProps XML strings into the flattened maps. Either part
    /// may be empty (absent in the package) → an empty map for that part.
    /// Malformed XML in either part is a hard [`ParseError::MalformedDocx`].
    pub(super) fn parse(core_xml: &str, app_xml: &str) -> Result<Self, ParseError> {
        Ok(Self {
            core: flatten_props(core_xml, "core.xml")?,
            app: flatten_props(app_xml, "app.xml")?,
        })
    }

    /// Look up a `core.xml` leaf by local name, `None` if absent.
    pub(super) fn core(&self, local: &str) -> Option<&str> {
        self.core.get(local).map(String::as_str)
    }

    /// Look up an `app.xml` leaf by local name, `None` if absent.
    pub(super) fn app(&self, local: &str) -> Option<&str> {
        self.app.get(local).map(String::as_str)
    }

    /// Parse an `app.xml` numeric leaf leniently: absent or non-numeric →
    /// `None` (never an error). OOXML emits these as plain integers
    /// (`<Pages>1</Pages>`); a producer that writes garbage just yields `None`.
    pub(super) fn app_u32(&self, local: &str) -> Option<u32> {
        self.app(local).and_then(|s| s.trim().parse::<u32>().ok())
    }
}

/// Walk one docProps part, collecting each *leaf* element's local name → its
/// trimmed text. Container elements (`coreProperties`, `Properties`) and
/// empty leaves are skipped — only elements that bracket non-empty text are
/// stored. On a name collision (shouldn't happen in well-formed docProps) the
/// first value wins.
fn flatten_props(xml: &str, part: &str) -> Result<HashMap<String, String>, ParseError> {
    let mut map: HashMap<String, String> = HashMap::new();
    let mut reader = Reader::from_str(xml);

    // The local name of the element currently open, plus its accumulating
    // text. `None` between elements / inside a container whose text we ignore.
    let mut current: Option<(String, String)> = None;

    loop {
        let event = reader
            .read_event()
            .map_err(|e| ParseError::MalformedDocx(format!("{part}: {e}")))?;
        match event {
            Event::Start(e) => {
                // Opening a new element resets the text accumulator. A nested
                // element (the vt: vectors in app.xml's HeadingPairs) replaces
                // `current`; its parent's text is whitespace-only anyway and
                // would be dropped by the empty-check on End.
                let name = local_name_str(e.name().as_ref());
                current = Some((name, String::new()));
            }
            Event::Text(t) => {
                if let Some((_, buf)) = current.as_mut() {
                    let decoded = t
                        .unescape()
                        .map_err(|e| ParseError::MalformedDocx(format!("{part}: {e}")))?;
                    buf.push_str(&decoded);
                }
            }
            Event::End(e) => {
                let name = local_name_str(e.name().as_ref());
                if let Some((open, text)) = current.take() {
                    // Only store when this End matches the element we opened
                    // (a leaf) and it bracketed non-empty text.
                    let trimmed = text.trim();
                    if open == name && !trimmed.is_empty() {
                        map.entry(open).or_insert_with(|| trimmed.to_string());
                    }
                }
            }
            // Empty elements (`<dc:subject/>`) carry no value → ignored.
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(map)
}

/// Strip any namespace prefix (`dc:title` → `title`, `cp:revision` →
/// `revision`), returning an owned `String`. Mirrors `body.rs` / `rels.rs`
/// prefix-stripping — robust to a producer that renames or omits prefixes.
fn local_name_str(qname: &[u8]) -> String {
    let local = match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    };
    String::from_utf8_lossy(local).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
        <cp:coreProperties
            xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
            xmlns:dc="http://purl.org/dc/elements/1.1/"
            xmlns:dcterms="http://purl.org/dc/terms/"
            xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
            <dc:title>Structured Sample Document</dc:title>
            <dc:subject/>
            <dc:creator>John Smith</dc:creator>
            <cp:keywords/>
            <dc:description>generated by python-docx</dc:description>
            <cp:lastModifiedBy>John Smith</cp:lastModifiedBy>
            <cp:revision>1</cp:revision>
            <dcterms:created xsi:type="dcterms:W3CDTF">2026-01-01T00:00:00Z</dcterms:created>
            <dcterms:modified xsi:type="dcterms:W3CDTF">2026-01-02T00:00:00Z</dcterms:modified>
            <cp:category/>
        </cp:coreProperties>"#;

    const APP: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
        <Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"
            xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
            <Template>Normal.dotm</Template>
            <TotalTime>0</TotalTime>
            <Pages>3</Pages>
            <Words>1518</Words>
            <Characters>8000</Characters>
            <Application>Microsoft Macintosh Word</Application>
            <DocSecurity>0</DocSecurity>
            <Lines>42</Lines>
            <Paragraphs>12</Paragraphs>
            <Manager/>
            <Company/>
            <AppVersion>14.0000</AppVersion>
        </Properties>"#;

    #[test]
    fn parses_core_leaves_skipping_empties() {
        let props = DocxProps::parse(CORE, "").unwrap();
        assert_eq!(props.core("title"), Some("Structured Sample Document"));
        assert_eq!(props.core("creator"), Some("John Smith"));
        assert_eq!(props.core("description"), Some("generated by python-docx"));
        assert_eq!(props.core("lastModifiedBy"), Some("John Smith"));
        assert_eq!(props.core("revision"), Some("1"));
        assert_eq!(props.core("created"), Some("2026-01-01T00:00:00Z"));
        assert_eq!(props.core("modified"), Some("2026-01-02T00:00:00Z"));
        // Empty placeholders (`<dc:subject/>`, `<cp:keywords/>`,
        // `<cp:category/>`) and absent fields (`dc:language`) → None.
        assert_eq!(props.core("subject"), None);
        assert_eq!(props.core("keywords"), None);
        assert_eq!(props.core("category"), None);
        assert_eq!(props.core("language"), None);
    }

    #[test]
    fn parses_app_leaves_and_numerics() {
        let props = DocxProps::parse("", APP).unwrap();
        assert_eq!(props.app("Application"), Some("Microsoft Macintosh Word"));
        assert_eq!(props.app("AppVersion"), Some("14.0000"));
        assert_eq!(props.app("Template"), Some("Normal.dotm"));
        assert_eq!(props.app_u32("Pages"), Some(3));
        assert_eq!(props.app_u32("Words"), Some(1518));
        assert_eq!(props.app_u32("Characters"), Some(8000));
        assert_eq!(props.app_u32("Lines"), Some(42));
        assert_eq!(props.app_u32("Paragraphs"), Some(12));
        assert_eq!(props.app_u32("TotalTime"), Some(0));
        assert_eq!(props.app_u32("DocSecurity"), Some(0));
        // Empty placeholders dropped.
        assert_eq!(props.app("Manager"), None);
        assert_eq!(props.app("Company"), None);
    }

    #[test]
    fn numeric_parse_is_lenient() {
        let props = DocxProps::parse("", APP).unwrap();
        // A present numeric parses; absent → None (not an error).
        assert_eq!(props.app_u32("Words"), Some(1518));
        assert_eq!(props.app_u32("Missing"), None);
        // A non-numeric value parses to None (AppVersion `14.0000` is a string
        // field; asking for it as u32 must not error, just yield None).
        assert_eq!(props.app_u32("AppVersion"), None);
    }

    #[test]
    fn empty_parts_are_empty_maps() {
        let props = DocxProps::parse("", "").unwrap();
        assert!(props.core.is_empty());
        assert!(props.app.is_empty());
        assert_eq!(props.core("title"), None);
        assert_eq!(props.app("Application"), None);
    }

    #[test]
    fn malformed_xml_is_an_error() {
        // Truncated mid-tag (unterminated start element) → quick-xml error,
        // surfaced as MalformedDocx — consistent with body.rs / rels.rs.
        let truncated = "<cp:coreProperties><dc:title";
        assert!(matches!(
            DocxProps::parse(truncated, ""),
            Err(ParseError::MalformedDocx(_))
        ));
        assert!(matches!(
            DocxProps::parse("", truncated),
            Err(ParseError::MalformedDocx(_))
        ));
    }
}

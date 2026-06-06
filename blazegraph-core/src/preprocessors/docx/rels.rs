//! DOCX relationships reader — `word/_rels/document.xml.rels`.
//!
//! The body's `<w:hyperlink r:id="rIdN">` elements carry only an opaque
//! relationship id. The actual URL (and whether the link is external) lives in
//! the relationships part `word/_rels/document.xml.rels`, a sibling zip entry:
//!
//! ```xml
//! <Relationships xmlns="…/relationships">
//!   <Relationship Id="rId4"
//!                 Type="…/officeDocument/2006/relationships/hyperlink"
//!                 Target="https://example.com/"
//!                 TargetMode="External"/>
//! </Relationships>
//! ```
//!
//! C2 only needs *hyperlink* relationships (those whose `Type` ends in
//! `/hyperlink`), mapped `Id → (Target, TargetMode)`. An external hyperlink
//! carries `TargetMode="External"`; an internal-part hyperlink (a link to
//! another part inside the package) omits it or sets `Internal` — out of scope
//! per contract decision 7, so we record the mode and let the body walk skip
//! non-external ones.
//!
//! C5 additionally resolves *header* / *footer* relationships (those whose
//! `Type` ends in `/header` / `/footer`), mapped `Id → Target` part path. The
//! body's `<w:sectPr>` carries `<w:headerReference r:id=…>` /
//! `<w:footerReference r:id=…>`; resolving the `r:id` here yields the
//! `word/headerN.xml` / `word/footerN.xml` part to read.

use std::collections::HashMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::super::md::types::ParseError;

/// One resolved hyperlink relationship: its target plus whether it points
/// outside the package. `mode` is the literal `TargetMode` attribute
/// (`"External"` / `"Internal"`); absent in the source → `None` (OOXML
/// defaults an omitted `TargetMode` to `Internal`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HyperlinkRel {
    /// `Target` attribute — the URL for an external hyperlink, or a part path
    /// for an internal one.
    pub(super) target: String,
    /// `TargetMode` attribute, verbatim. `Some("External")` is the only case
    /// C2 resolves to an `ExternalRef`.
    pub(super) mode: Option<String>,
}

impl HyperlinkRel {
    /// True iff this relationship is an external link (`TargetMode="External"`,
    /// case-insensitive). Only external hyperlinks become `ExternalRef`s; an
    /// internal-part link is out of scope (contract decision 7).
    pub(super) fn is_external(&self) -> bool {
        self.mode
            .as_deref()
            .is_some_and(|m| m.eq_ignore_ascii_case("External"))
    }
}

/// The `Type` suffix that identifies a hyperlink relationship. Matched as a
/// suffix so we're agnostic to the (stable) schema-URL prefix.
const HYPERLINK_TYPE_SUFFIX: &str = "/hyperlink";

/// The `Type` suffixes for header / footer part relationships (C5). Matched as
/// suffixes for the same prefix-agnostic reason as the hyperlink suffix.
const HEADER_TYPE_SUFFIX: &str = "/header";
const FOOTER_TYPE_SUFFIX: &str = "/footer";

/// Which chrome part a header/footer relationship points to. Drives the node
/// type the body walk emits for the referenced part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChromeKind {
    Header,
    Footer,
}

/// One resolved header/footer relationship: the part path it targets plus
/// whether it's a header or footer. The `r:id` on a `<w:headerReference>` /
/// `<w:footerReference>` resolves to one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChromeRel {
    /// `Target` attribute — the part path, relative to `word/` (e.g.
    /// `header1.xml`). The body resolves it against the `word/` directory.
    pub(super) target: String,
    /// Header vs. footer, from the relationship `Type` suffix.
    pub(super) kind: ChromeKind,
}

/// Parse `word/_rels/document.xml.rels` into a `rId → ChromeRel` map, keeping
/// only relationships whose `Type` ends in `/header` or `/footer` (C5).
///
/// This is the header/footer twin of [`parse_hyperlink_rels`]: a single rels
/// part carries every relationship type; the two parsers each filter to their
/// own. A missing / empty rels part — or a document with no header/footer
/// parts — yields an empty map. Malformed XML is a hard
/// [`ParseError::MalformedDocx`].
pub(super) fn parse_chrome_rels(rels_xml: &str) -> Result<HashMap<String, ChromeRel>, ParseError> {
    let mut map: HashMap<String, ChromeRel> = HashMap::new();
    let mut reader = Reader::from_str(rels_xml);

    loop {
        let event = reader
            .read_event()
            .map_err(|e| ParseError::MalformedDocx(format!("document.xml.rels: {e}")))?;
        match event {
            Event::Empty(e) | Event::Start(e)
                if local_name(e.name().as_ref()) == b"Relationship" =>
            {
                if let Some((id, rel)) = read_chrome_relationship(&e) {
                    map.insert(id, rel);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(map)
}

/// Extract `(Id, ChromeRel)` from one `<Relationship>` element, returning
/// `None` for non-header/footer relationships and for entries missing `Id` or
/// `Target`.
fn read_chrome_relationship(e: &BytesStart) -> Option<(String, ChromeRel)> {
    let ty = attr_value(e, b"Type")?;
    let kind = if ty.ends_with(HEADER_TYPE_SUFFIX) {
        ChromeKind::Header
    } else if ty.ends_with(FOOTER_TYPE_SUFFIX) {
        ChromeKind::Footer
    } else {
        return None;
    };
    let id = attr_value(e, b"Id")?;
    let target = attr_value(e, b"Target")?;
    Some((id, ChromeRel { target, kind }))
}

/// Parse `word/_rels/document.xml.rels` into a `rId → HyperlinkRel` map,
/// keeping only relationships whose `Type` ends in `/hyperlink`.
///
/// A missing or empty rels part yields an empty map (a document with no
/// `r:id` hyperlinks needs no rels resolution) — callers pass
/// [`Default::default`] / `""` in that case. Malformed XML is a hard
/// [`ParseError::MalformedDocx`], consistent with the rest of the channel.
pub(super) fn parse_hyperlink_rels(
    rels_xml: &str,
) -> Result<HashMap<String, HyperlinkRel>, ParseError> {
    let mut map: HashMap<String, HyperlinkRel> = HashMap::new();
    let mut reader = Reader::from_str(rels_xml);

    loop {
        let event = reader
            .read_event()
            .map_err(|e| ParseError::MalformedDocx(format!("document.xml.rels: {e}")))?;
        match event {
            // `<Relationship .../>` is normally an empty element; tolerate the
            // Start form too in case a producer nests anything inside.
            Event::Empty(e) | Event::Start(e)
                if local_name(e.name().as_ref()) == b"Relationship" =>
            {
                if let Some((id, rel)) = read_relationship(&e) {
                    map.insert(id, rel);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(map)
}

/// Extract `(Id, HyperlinkRel)` from one `<Relationship>` element, returning
/// `None` for non-hyperlink relationships (styles, theme, fontTable, …) and
/// for malformed entries missing `Id` or `Target`.
fn read_relationship(e: &BytesStart) -> Option<(String, HyperlinkRel)> {
    let ty = attr_value(e, b"Type")?;
    if !ty.ends_with(HYPERLINK_TYPE_SUFFIX) {
        return None;
    }
    let id = attr_value(e, b"Id")?;
    let target = attr_value(e, b"Target")?;
    let mode = attr_value(e, b"TargetMode");
    Some((id, HyperlinkRel { target, mode }))
}

// ---- XML helpers (rels parts are unprefixed, but mirror body.rs for safety) -

/// Strip any namespace prefix (`a:Relationship` → `Relationship`). Rels parts
/// use an unprefixed default namespace, but matching on the local name is
/// robust to a producer that adds one.
fn local_name(qname: &[u8]) -> &[u8] {
    match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    }
}

/// Read an attribute value by (possibly prefixed) name, matching on the local
/// name. Rels attributes (`Id`, `Type`, `Target`, `TargetMode`) are unprefixed.
fn attr_value(e: &BytesStart, name: &[u8]) -> Option<String> {
    let want = local_name(name);
    e.attributes().flatten().find_map(|a| {
        if local_name(a.key.as_ref()) == want {
            a.unescape_value()
                .ok()
                .map(|c| c.into_owned())
                .or_else(|| Some(String::from_utf8_lossy(&a.value).into_owned()))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_external_and_internal_hyperlinks() {
        // Two hyperlink rels (one external, one internal-part) plus a
        // non-hyperlink rel that must be ignored.
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
        <Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
            <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/>
            <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="media/image1.png"/>
        </Relationships>"#;
        let map = parse_hyperlink_rels(xml).unwrap();
        // Only the two hyperlink rels are kept (styles.xml is dropped).
        assert_eq!(map.len(), 2);

        let ext = &map["rId2"];
        assert_eq!(ext.target, "https://example.com/");
        assert!(ext.is_external(), "TargetMode=External → external");

        let internal = &map["rId3"];
        assert_eq!(internal.target, "media/image1.png");
        assert!(
            !internal.is_external(),
            "no TargetMode → internal-part link (skipped downstream)"
        );
        assert!(internal.mode.is_none());
    }

    #[test]
    fn target_mode_match_is_case_insensitive() {
        let xml = r#"<Relationships>
            <Relationship Id="rId9" Type="http://x/relationships/hyperlink" Target="https://a/" TargetMode="external"/>
        </Relationships>"#;
        let map = parse_hyperlink_rels(xml).unwrap();
        assert!(
            map["rId9"].is_external(),
            "TargetMode matching is case-insensitive"
        );
    }

    #[test]
    fn empty_rels_is_empty_map() {
        assert!(parse_hyperlink_rels("").unwrap().is_empty());
        let xml = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#;
        assert!(parse_hyperlink_rels(xml).unwrap().is_empty());
    }

    #[test]
    fn non_hyperlink_rels_are_ignored() {
        let xml = r#"<Relationships>
            <Relationship Id="rId1" Type="http://x/relationships/officeDocument" Target="word/document.xml"/>
            <Relationship Id="rId2" Type="http://x/relationships/theme" Target="theme/theme1.xml"/>
        </Relationships>"#;
        assert!(
            parse_hyperlink_rels(xml).unwrap().is_empty(),
            "non-hyperlink relationships are dropped"
        );
    }

    #[test]
    fn malformed_relationship_missing_target_skipped() {
        // A hyperlink rel missing Target is dropped (not a hard error).
        let xml = r#"<Relationships>
            <Relationship Id="rId4" Type="http://x/relationships/hyperlink" TargetMode="External"/>
            <Relationship Id="rId5" Type="http://x/relationships/hyperlink" Target="https://ok/" TargetMode="External"/>
        </Relationships>"#;
        let map = parse_hyperlink_rels(xml).unwrap();
        assert_eq!(map.len(), 1, "only the well-formed rel survives");
        assert!(map.contains_key("rId5"));
    }

    #[test]
    fn malformed_xml_is_an_error() {
        let xml = "<Relationships><Relationship Id=\"rId1\""; // truncated
        assert!(matches!(
            parse_hyperlink_rels(xml),
            Err(ParseError::MalformedDocx(_))
        ));
    }

    // ---- C5 header/footer rels --------------------------------------------

    #[test]
    fn parses_header_and_footer_rels() {
        // The two chrome rels python-docx emits, alongside non-chrome rels
        // (styles, hyperlink) that must be ignored by the chrome parser.
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
        <Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
            <Relationship Id="rId8" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/>
            <Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>
            <Relationship Id="rId10" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/>
        </Relationships>"#;
        let map = parse_chrome_rels(xml).unwrap();
        // Only the header + footer rels are kept (styles / hyperlink dropped).
        assert_eq!(map.len(), 2);

        let h = &map["rId9"];
        assert_eq!(h.target, "header1.xml");
        assert_eq!(h.kind, ChromeKind::Header);

        let f = &map["rId10"];
        assert_eq!(f.target, "footer1.xml");
        assert_eq!(f.kind, ChromeKind::Footer);
    }

    #[test]
    fn chrome_parser_ignores_hyperlink_rels() {
        // A rels part with only a hyperlink → empty chrome map (and vice
        // versa: the hyperlink parser ignores chrome rels — both filters are
        // independent over the one shared part).
        let xml = r#"<Relationships>
            <Relationship Id="rId2" Type="http://x/relationships/hyperlink" Target="https://a/" TargetMode="External"/>
        </Relationships>"#;
        assert!(parse_chrome_rels(xml).unwrap().is_empty());

        let xml = r#"<Relationships>
            <Relationship Id="rId9" Type="http://x/relationships/header" Target="header1.xml"/>
        </Relationships>"#;
        assert!(parse_hyperlink_rels(xml).unwrap().is_empty());
    }

    #[test]
    fn chrome_rel_missing_target_skipped() {
        let xml = r#"<Relationships>
            <Relationship Id="rId9" Type="http://x/relationships/header"/>
            <Relationship Id="rId10" Type="http://x/relationships/footer" Target="footer1.xml"/>
        </Relationships>"#;
        let map = parse_chrome_rels(xml).unwrap();
        assert_eq!(map.len(), 1, "only the well-formed chrome rel survives");
        assert!(map.contains_key("rId10"));
    }

    #[test]
    fn empty_chrome_rels_is_empty_map() {
        assert!(parse_chrome_rels("").unwrap().is_empty());
    }
}

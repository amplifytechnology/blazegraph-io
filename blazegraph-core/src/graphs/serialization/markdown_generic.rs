//! Generic markdown emitter — `DocumentGraph` → plain markdown string.
//!
//! Sibling to [`super::markdown`] (which is the bgraph.md emitter).
//! Where `markdown.rs` produces the self-describing round-trip artifact
//! with embedded fences, this module produces *plain* markdown — the
//! kind a markdown viewer renders directly.
//!
//! ## Output shape
//!
//! 1. Optional YAML frontmatter — emitted only when
//!    `document_metadata` has content (any of `title`, `author`,
//!    `date`, `description`, `tags`, `draft`, `extras` populated).
//! 2. Body — for each node in `text_order` ascending order (skipping
//!    the Document root):
//!    - `Section` → `"#".repeat(depth) + " " + text` (capped at 6
//!      hashes per the markdown limit; the metadata still carries the
//!      true depth, see `markdown.rs` for the equivalent emitter rule).
//!    - `Paragraph` / `CodeBlock` / `List` / `Blockquote` / `Table` →
//!      `text` verbatim.
//!    - `Header` / `Footer` / `Margin` → panic (these PDF-only
//!      variants are unrepresentable in plain markdown source; the
//!      CLI pre-emit check at
//!      `blazegraph-cli/src/main.rs::check_generic_md_compatible`
//!      should reject the graph before we ever reach here).
//!    - `Document` → skipped (synthetic root).
//! 3. Blocks separated by a single blank line.
//!
//! Trailing newline: the output ends with exactly one `\n` so it
//! round-trips byte-identically with the parser's slice convention
//! (which trims the trailing `\n` from each block's range and then
//! `\n\n`-joins them on the way back out).
//!
//! ## Panics
//!
//! Panics on `Header` / `Footer` / `Margin` variants — those don't
//! exist in generic markdown source. The CLI calls
//! `check_generic_md_compatible` before invoking this emitter, so
//! the panic is unreachable in normal user flow. It's a defense-
//! in-depth safety net for in-process callers who skip the check.

use crate::types::*;
use std::collections::BTreeMap;

/// Emit a `DocumentGraph` to plain markdown.
///
/// See the module docs for the output shape contract and the panic
/// conditions.
pub fn emit_markdown(graph: &DocumentGraph) -> String {
    let mut out = String::new();

    // 1. Frontmatter (optional). Closer `---` is followed by a
    //    single `\n` (not a blank line); the parser's
    //    `extract_frontmatter` strips exactly the frontmatter block +
    //    closer newline and returns the body slice — so emitting
    //    frontmatter + body directly (no extra blank) matches what
    //    the parser produced on the way in.
    if let Some(frontmatter) = emit_frontmatter(&graph.document_info.document_metadata) {
        out.push_str(&frontmatter);
        out.push('\n');
    }

    // 2. Body — walk nodes by text_order ascending.
    let mut nodes: Vec<&DocumentNode> = graph
        .nodes
        .values()
        .filter(|n| n.text_order.is_some())
        .collect();
    nodes.sort_by_key(|n| n.text_order.expect("filtered above"));

    let mut body_parts: Vec<String> = Vec::new();
    for node in nodes {
        if let Some(chunk) = emit_node(node) {
            body_parts.push(chunk);
        }
    }
    out.push_str(&body_parts.join("\n\n"));

    // 3. Ensure single trailing newline.
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Emit YAML frontmatter from `DocumentMetadata` when there's content
/// to carry. Returns `None` when all canonical fields are unset and the
/// `md` namespace's flat fields + `extras` are empty (so we don't write
/// an empty `---\n---\n` block).
///
/// Field order: `title`, `author`, `date`, `description`, `draft`,
/// `tags`, `categories`, then `md.extras` in sorted-key order. Order is
/// stable so the emitter is byte-deterministic and round-trips cleanly
/// through the frontmatter pre-pass.
///
/// CR-57: `date` is sourced from canonical `metadata.created`;
/// `draft` / `tags` / `categories` / extras are sourced from
/// `metadata.md`.
fn emit_frontmatter(metadata: &DocumentMetadata) -> Option<String> {
    if !has_frontmatter_content(metadata) {
        return None;
    }

    let mut lines: Vec<String> = Vec::new();
    lines.push("---".to_string());

    if let Some(ref title) = metadata.title {
        lines.push(emit_scalar_field("title", title));
    }
    if let Some(ref author) = metadata.author {
        lines.push(emit_scalar_field("author", author));
    }
    // `date` in MD frontmatter is the canonical `created` slot
    // (per `09-metadata-first-class.md` § Notes on `created`). Round-trip
    // semantics: parse → canonical `created` → emit `date`.
    if let Some(ref created) = metadata.created {
        lines.push(emit_scalar_field("date", created));
    }
    if let Some(ref description) = metadata.description {
        lines.push(emit_scalar_field("description", description));
    }
    if let Some(md_ns) = metadata.md.as_ref() {
        if !md_ns.tags.is_empty() {
            lines.push(emit_tags(&md_ns.tags));
        }
        if let Some(draft) = md_ns.draft {
            lines.push(format!("draft: {draft}"));
        }
        if !md_ns.categories.is_empty() {
            lines.push(emit_categories(&md_ns.categories));
        }
        emit_extras(&md_ns.extras, &mut lines);
    }

    lines.push("---".to_string());
    Some(lines.join("\n"))
}

fn has_frontmatter_content(metadata: &DocumentMetadata) -> bool {
    if metadata.title.is_some()
        || metadata.author.is_some()
        || metadata.created.is_some()
        || metadata.description.is_some()
    {
        return true;
    }
    if let Some(md_ns) = metadata.md.as_ref() {
        if md_ns.draft.is_some()
            || !md_ns.tags.is_empty()
            || !md_ns.categories.is_empty()
            || !md_ns.extras.is_empty()
        {
            return true;
        }
    }
    false
}

fn emit_categories(categories: &[String]) -> String {
    let items: Vec<String> = categories
        .iter()
        .map(|c| {
            if needs_yaml_quoting(c) {
                let escaped = c.replace('\\', r"\\").replace('"', r#"\""#);
                format!("\"{escaped}\"")
            } else {
                c.clone()
            }
        })
        .collect();
    format!("categories: [{}]", items.join(", "))
}

/// Emit one YAML scalar field. If the value needs quoting (contains
/// special chars, leading/trailing whitespace, or looks like a YAML
/// keyword), we wrap it in double-quotes; otherwise plain.
fn emit_scalar_field(key: &str, value: &str) -> String {
    if needs_yaml_quoting(value) {
        // Escape inner double-quotes + backslashes for YAML
        // double-quoted strings.
        let escaped = value.replace('\\', r"\\").replace('"', r#"\""#);
        format!("{key}: \"{escaped}\"")
    } else {
        format!("{key}: {value}")
    }
}

/// Conservative quoting predicate. Returns true for any string we
/// want to wrap in `"..."` to be safe on the YAML parser's strict
/// modes (gray_matter's yaml-rust2 backend tolerates most plain
/// scalars but flow indicators / leading whitespace cause grief).
fn needs_yaml_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if s.starts_with(' ') || s.ends_with(' ') {
        return true;
    }
    // YAML flow indicators that change scalar interpretation.
    s.chars().any(|c| {
        matches!(
            c,
            ':' | '#' | '&' | '*' | '!' | '|' | '>' | '\'' | '"' | '%' | '@' | '`' | '\n' | '\t'
        )
    }) || matches!(s, "true" | "false" | "null" | "yes" | "no" | "on" | "off")
}

fn emit_tags(tags: &[String]) -> String {
    // Flow-style array: `tags: [a, b, c]`. Items needing quoting
    // are wrapped. Stable, compact, and the inverse of how the
    // pre-pass reads them.
    let items: Vec<String> = tags
        .iter()
        .map(|t| {
            if needs_yaml_quoting(t) {
                let escaped = t.replace('\\', r"\\").replace('"', r#"\""#);
                format!("\"{escaped}\"")
            } else {
                t.clone()
            }
        })
        .collect();
    format!("tags: [{}]", items.join(", "))
}

fn emit_extras(extras: &BTreeMap<String, serde_json::Value>, lines: &mut Vec<String>) {
    for (key, value) in extras {
        // JSON happens to be valid YAML flow syntax for scalars,
        // arrays, and objects — `serde_json::to_string(value)` gives
        // us a one-line YAML-compatible representation for free.
        let serialized =
            serde_json::to_string(value).expect("serde_json::Value is always serializable");
        lines.push(format!("{key}: {serialized}"));
    }
}

/// Emit one node. Returns `None` for nodes we skip (Document root).
///
/// Panics on Header/Footer/Margin — see module docs for rationale.
fn emit_node(node: &DocumentNode) -> Option<String> {
    match node.node_type.as_str() {
        "Document" => None, // synthetic root; not a content node
        "Section" => {
            let depth = node.location.semantic.depth as usize;
            let prefix = heading_prefix(depth);
            Some(format!("{prefix} {text}", text = node.content.text))
        }
        "Paragraph" | "CodeBlock" | "List" | "Blockquote" | "Table" => {
            Some(node.content.text.clone())
        }
        "Header" | "Footer" | "Margin" => {
            panic!(
                "emit_markdown (generic): variant '{other}' is PDF-only and not \
                 representable in plain markdown source; the CLI's \
                 `check_generic_md_compatible` should reject the graph before \
                 reaching the emitter",
                other = node.node_type,
            );
        }
        other => {
            panic!(
                "emit_markdown (generic): unknown variant '{other}'; schema added \
                 a variant without updating this emitter",
            );
        }
    }
}

/// Markdown heading prefix for `depth`: `#` to `######` for depths
/// 1..=6, capped at `######` for depth ≥ 7. Mirrors the
/// `markdown.rs::heading_prefix` rule.
fn heading_prefix(depth: usize) -> &'static str {
    match depth {
        0 | 1 => "#",
        2 => "##",
        3 => "###",
        4 => "####",
        5 => "#####",
        _ => "######",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use uuid::Uuid;

    /// Build a minimal graph with a Document root + the supplied body
    /// nodes. `nodes_in` is `(node_type, text, depth, text_order)`.
    /// Lifted from `markdown.rs::tests::build_graph` so this test
    /// module is self-contained.
    fn build_graph(nodes_in: Vec<(&str, &str, u32, u32)>) -> DocumentGraph {
        let root_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"generic-test-root");
        let mut nodes = HashMap::new();
        let mut child_ids = Vec::new();

        for (node_type, text, depth, text_order) in &nodes_in {
            let id = Uuid::new_v5(
                &Uuid::NAMESPACE_DNS,
                format!("generic-test:{}", text_order).as_bytes(),
            );
            child_ids.push(id);
            nodes.insert(
                id,
                DocumentNode {
                    id,
                    node_type: node_type.to_string(),
                    location: NodeLocation {
                        semantic: SemanticLocation {
                            path: format!("{}", text_order + 1),
                            depth: *depth,
                            breadcrumbs: Vec::new(),
                        },
                        physical: None,
                    },
                    text_order: Some(*text_order),
                    content: NodeContent {
                        text: text.to_string(),
                    },
                    style_info: None,
                    message_metadata: None,
                    token_count: 1,
                    parent: Some(root_id),
                    children: Vec::new(),
                },
            );
        }

        nodes.insert(
            root_id,
            DocumentNode {
                id: root_id,
                node_type: "Document".to_string(),
                location: NodeLocation {
                    semantic: SemanticLocation {
                        path: String::new(),
                        depth: 0,
                        breadcrumbs: Vec::new(),
                    },
                    physical: None,
                },
                text_order: None,
                content: NodeContent {
                    text: String::new(),
                },
                style_info: None,
                message_metadata: None,
                token_count: 0,
                parent: None,
                children: child_ids,
            },
        );

        DocumentGraph {
            nodes,
            document_info: DocumentInfo {
                root_id,
                document_metadata: DocumentMetadata::default(),
                bookmark_data: None,
                parse_provenance: None, // generic emitter doesn't need provenance
                topology: None,
                source_identity: None,
                supersedes: None,
            },
            structural_profile: StructuralProfile::default(),
        }
    }

    #[test]
    fn emit_section_uses_hash_prefix() {
        let graph = build_graph(vec![("Section", "Hello", 1, 0)]);
        let md = emit_markdown(&graph);
        assert!(
            md.contains("# Hello"),
            "depth-1 Section should emit `# Hello`; got:\n{md}"
        );
    }

    #[test]
    fn emit_paragraph_text_verbatim() {
        let graph = build_graph(vec![("Paragraph", "Hello world.", 1, 0)]);
        let md = emit_markdown(&graph);
        assert!(
            md.contains("Hello world."),
            "Paragraph body should be verbatim; got:\n{md}"
        );
        assert!(
            !md.contains("```"),
            "Paragraph emit should not introduce code fences; got:\n{md}"
        );
    }

    #[test]
    fn emit_frontmatter_when_metadata_present() {
        let mut graph = build_graph(vec![("Section", "Hi", 1, 0)]);
        graph.document_info.document_metadata.title = Some("Test Doc".to_string());
        graph.document_info.document_metadata.author = Some("Marcus".to_string());
        // Strong-convention `tags` lives under the `md` namespace post-CR-57.
        graph.document_info.document_metadata.md = Some(MdMetadata {
            tags: vec!["rust".to_string(), "b6".to_string()],
            ..Default::default()
        });
        let md = emit_markdown(&graph);
        assert!(
            md.starts_with("---\n"),
            "frontmatter should open with `---\\n`; got:\n{md}"
        );
        assert!(
            md.contains("title: Test Doc"),
            "title should round-trip; got:\n{md}"
        );
        assert!(
            md.contains("tags: [rust, b6]"),
            "tags should round-trip as flow array; got:\n{md}"
        );
    }

    #[test]
    fn emit_skips_frontmatter_when_metadata_empty() {
        let graph = build_graph(vec![("Paragraph", "Body.", 1, 0)]);
        let md = emit_markdown(&graph);
        assert!(
            !md.starts_with("---"),
            "empty metadata should not produce a frontmatter block; got:\n{md}"
        );
    }

    #[test]
    fn emit_codeblock_text_verbatim() {
        let raw = "```rust\nfn main() {}\n```";
        let graph = build_graph(vec![("CodeBlock", raw, 1, 0)]);
        let md = emit_markdown(&graph);
        assert!(
            md.contains(raw),
            "CodeBlock body should be emitted verbatim (fence + body); got:\n{md}"
        );
    }

    #[test]
    #[should_panic(expected = "PDF-only")]
    fn emit_panics_on_header_variant() {
        let graph = build_graph(vec![("Header", "running header", 1, 0)]);
        let _ = emit_markdown(&graph);
    }

    #[test]
    #[should_panic(expected = "PDF-only")]
    fn emit_panics_on_footer_variant() {
        let graph = build_graph(vec![("Footer", "running footer", 1, 0)]);
        let _ = emit_markdown(&graph);
    }

    #[test]
    #[should_panic(expected = "PDF-only")]
    fn emit_panics_on_margin_variant() {
        let graph = build_graph(vec![("Margin", "marginalia", 1, 0)]);
        let _ = emit_markdown(&graph);
    }

    #[test]
    fn emit_extras_pass_through_in_sorted_order() {
        let mut graph = build_graph(vec![("Paragraph", "body", 1, 0)]);
        // `extras` lives under the md namespace post-CR-57.
        let mut md_ns = MdMetadata::default();
        md_ns
            .extras
            .insert("zeta".to_string(), serde_json::Value::String("z".into()));
        md_ns
            .extras
            .insert("alpha".to_string(), serde_json::Value::Number(7.into()));
        graph.document_info.document_metadata.md = Some(md_ns);
        let md = emit_markdown(&graph);
        // BTreeMap iteration → alphabetical: alpha before zeta.
        let alpha_pos = md.find("alpha: 7").expect("alpha emitted");
        let zeta_pos = md.find("zeta: \"z\"").expect("zeta emitted");
        assert!(
            alpha_pos < zeta_pos,
            "extras should emit in sorted-key order; got:\n{md}"
        );
    }
}

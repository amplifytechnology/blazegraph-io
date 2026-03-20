"""Test render output readability and formatting.

Uses both a small hand-built fixture (for precise output verification)
and the real Shannon graph (for full-document render checks).
"""

from __future__ import annotations

from blazegraphio.types import (
    BlazeGraph,
    BoundingBox,
    DocumentAnalysis,
    DocumentInfo,
    DocumentMetadata,
    DocumentNode,
    DepthDistribution,
    NodeContent,
    NodeLocation,
    NodeTypeDistribution,
    PhysicalLocation,
    SemanticLocation,
    StructuralProfile,
    TokenDistribution,
)


def _make_mini_graph() -> BlazeGraph:
    """Build a small graph: Document -> Section -> 2 Paragraphs.

    Mirrors the Shannon paper's structure for testing render output.
    """
    doc_node = DocumentNode(
        id="doc-001",
        node_type="Document",
        location=NodeLocation(
            semantic=SemanticLocation(path="", depth=0, breadcrumbs=["shannon1948.dvi"]),
            physical=None,
        ),
        text_order=None,
        content=NodeContent(text="Document"),
        token_count=0,
        parent=None,
        children=["sec-001"],
    )

    section_node = DocumentNode(
        id="sec-001",
        node_type="Section",
        location=NodeLocation(
            semantic=SemanticLocation(
                path="1",
                depth=1,
                breadcrumbs=["shannon1948.dvi", "A Mathematical Theory of Communication"],
            ),
            physical=PhysicalLocation(
                page=1,
                bounding_box=BoundingBox(x=181.0, y=127.9, width=249.3, height=8.0),
            ),
        ),
        text_order=0,
        content=NodeContent(text="A Mathematical Theory of Communication"),
        token_count=9,
        parent="doc-001",
        children=["para-001", "para-002"],
    )

    para1 = DocumentNode(
        id="para-001",
        node_type="Paragraph",
        location=NodeLocation(
            semantic=SemanticLocation(
                path="1.1",
                depth=2,
                breadcrumbs=[
                    "shannon1948.dvi",
                    "A Mathematical Theory of Communication",
                ],
            ),
            physical=PhysicalLocation(
                page=1,
                bounding_box=BoundingBox(x=91.9, y=585.9, width=427.5, height=164.2),
            ),
        ),
        text_order=1,
        content=NodeContent(
            text=(
                "The fundamental problem of communication is that of reproducing at one point "
                "either exactly or approximately a message selected at another point."
            )
        ),
        token_count=25,
        parent="sec-001",
        children=[],
    )

    para2 = DocumentNode(
        id="para-002",
        node_type="Paragraph",
        location=NodeLocation(
            semantic=SemanticLocation(
                path="1.2",
                depth=2,
                breadcrumbs=[
                    "shannon1948.dvi",
                    "A Mathematical Theory of Communication",
                ],
            ),
            physical=PhysicalLocation(
                page=1,
                bounding_box=BoundingBox(x=91.9, y=750.0, width=427.5, height=80.0),
            ),
        ),
        text_order=2,
        content=NodeContent(
            text=(
                "Frequently the messages have meaning; that is they refer to or are correlated "
                "according to some system with certain physical or conceptual entities."
            )
        ),
        token_count=26,
        parent="sec-001",
        children=[],
    )

    nodes = [doc_node, section_node, para1, para2]
    raw = {"schema_version": "0.2.0", "nodes": [], "document_info": {}, "structural_profile": {}}

    return BlazeGraph(
        schema_version="0.2.0",
        nodes=nodes,
        document_info=DocumentInfo(
            root_id="doc-001",
            document_metadata=DocumentMetadata(page_count=55),
            document_analysis=DocumentAnalysis(),
        ),
        structural_profile=StructuralProfile(total_nodes=4, total_tokens=60),
        _raw=raw,
    )


class TestRenderPlain:
    """Test plain render (no flags)."""

    def test_section_render(self) -> None:
        graph = _make_mini_graph()
        section = graph.get_node("sec-001")
        output = section.render(graph)

        print("\n--- Plain render (section) ---")
        print(output)
        print("--- end ---\n")

        lines = output.split("\n\n")
        assert len(lines) == 3
        assert lines[0] == "A Mathematical Theory of Communication"
        assert lines[1].startswith("The fundamental problem")
        assert lines[2].startswith("Frequently the messages")

    def test_no_trailing_whitespace(self) -> None:
        graph = _make_mini_graph()
        section = graph.get_node("sec-001")
        output = section.render(graph)
        for line in output.split("\n"):
            assert line == line.rstrip(), f"Trailing whitespace found: {line!r}"

    def test_no_extra_blank_lines(self) -> None:
        graph = _make_mini_graph()
        section = graph.get_node("sec-001")
        output = section.render(graph)
        assert "\n\n\n" not in output

    def test_paragraph_render_standalone(self) -> None:
        graph = _make_mini_graph()
        para = graph.get_node("para-001")
        output = para.render(graph)
        assert output == (
            "The fundamental problem of communication is that of reproducing at one point "
            "either exactly or approximately a message selected at another point."
        )


class TestRenderBreadcrumbs:
    """Test render with breadcrumbs=True."""

    def test_section_with_breadcrumbs(self) -> None:
        graph = _make_mini_graph()
        section = graph.get_node("sec-001")
        output = section.render(graph, breadcrumbs=True)

        print("\n--- Breadcrumbs render ---")
        print(output)
        print("--- end ---\n")

        lines = output.split("\n\n")
        assert len(lines) == 3
        assert lines[0] == "[shannon1948.dvi > A Mathematical Theory of Communication]"
        # Paragraphs don't show breadcrumbs
        assert lines[1].startswith("The fundamental problem")
        assert not lines[1].startswith("[")

    def test_paragraph_ignores_breadcrumbs(self) -> None:
        graph = _make_mini_graph()
        para = graph.get_node("para-001")
        output = para.render(graph, breadcrumbs=True)
        # Paragraphs never show breadcrumbs
        assert not output.startswith("[")
        assert output.startswith("The fundamental problem")


class TestRenderNodeTypes:
    """Test render with node_types=True."""

    def test_section_with_types(self) -> None:
        graph = _make_mini_graph()
        section = graph.get_node("sec-001")
        output = section.render(graph, node_types=True)

        print("\n--- Node types render ---")
        print(output)
        print("--- end ---\n")

        lines = output.split("\n\n")
        assert len(lines) == 3
        assert lines[0] == "[Section] A Mathematical Theory of Communication"
        assert lines[1].startswith("[Paragraph] The fundamental")
        assert lines[2].startswith("[Paragraph] Frequently")

    def test_paragraph_with_types(self) -> None:
        graph = _make_mini_graph()
        para = graph.get_node("para-001")
        output = para.render(graph, node_types=True)
        assert output.startswith("[Paragraph] The fundamental")


class TestRenderBoth:
    """Test render with breadcrumbs=True and node_types=True."""

    def test_combined_flags(self) -> None:
        graph = _make_mini_graph()
        section = graph.get_node("sec-001")
        output = section.render(graph, breadcrumbs=True, node_types=True)

        print("\n--- Combined render ---")
        print(output)
        print("--- end ---\n")

        lines = output.split("\n\n")
        assert len(lines) == 3
        assert lines[0] == "[Section | shannon1948.dvi > A Mathematical Theory of Communication]"
        assert lines[1].startswith("[Paragraph] The fundamental")
        assert lines[2].startswith("[Paragraph] Frequently")

    def test_paragraphs_get_type_only(self) -> None:
        graph = _make_mini_graph()
        para = graph.get_node("para-001")
        output = para.render(graph, breadcrumbs=True, node_types=True)
        assert output.startswith("[Paragraph] The fundamental")
        # Should NOT have breadcrumbs
        assert "shannon1948" not in output


class TestFullDocumentRender:
    """Test graph.render() on the full Shannon fixture."""

    def test_full_render_produces_text(self, shannon_graph: BlazeGraph) -> None:
        output = shannon_graph.render()
        assert len(output) > 1000  # Should be substantial
        assert "Communication" in output

        # Print first 500 chars to visually verify readability
        print("\n--- Full document render (first 500 chars) ---")
        print(output[:500])
        print("--- end ---\n")

    def test_full_render_no_triple_newlines(self, shannon_graph: BlazeGraph) -> None:
        output = shannon_graph.render()
        assert "\n\n\n" not in output

    def test_full_render_with_breadcrumbs(self, shannon_graph: BlazeGraph) -> None:
        output = shannon_graph.render(breadcrumbs=True)
        assert "[shannon1948.dvi" in output

    def test_full_render_with_node_types(self, shannon_graph: BlazeGraph) -> None:
        output = shannon_graph.render(node_types=True)
        assert "[Section]" in output
        assert "[Paragraph]" in output

    def test_document_node_render(self, shannon_graph: BlazeGraph) -> None:
        """Document root render should produce the same as graph.render()."""
        root_output = shannon_graph.root.render(shannon_graph)
        graph_output = shannon_graph.render()
        assert root_output == graph_output

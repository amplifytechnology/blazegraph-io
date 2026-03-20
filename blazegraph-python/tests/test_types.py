"""Test deserialization of shannon_graph.json into fully typed BlazeGraph."""

from __future__ import annotations

from blazegraphio.types import (
    BlazeGraph,
    BoundingBox,
    DocumentInfo,
    DocumentNode,
    NodeContent,
    NodeLocation,
    PhysicalLocation,
    SemanticLocation,
    StructuralProfile,
)


class TestBlazeGraphDeserialization:
    """Deserialize the real Shannon fixture and verify all typed fields."""

    def test_top_level_fields(self, shannon_graph: BlazeGraph) -> None:
        assert shannon_graph.schema_version == "0.2.0"
        assert len(shannon_graph.nodes) > 0
        assert isinstance(shannon_graph.document_info, DocumentInfo)
        assert isinstance(shannon_graph.structural_profile, StructuralProfile)

    def test_repr(self, shannon_graph: BlazeGraph) -> None:
        r = repr(shannon_graph)
        assert "BlazeGraph" in r
        assert "nodes" in r
        assert "v0.2.0" in r

    def test_node_count(self, shannon_graph: BlazeGraph) -> None:
        # Shannon paper fixture has 95 nodes
        assert len(shannon_graph.nodes) == 95

    def test_root_node(self, shannon_graph: BlazeGraph) -> None:
        root = shannon_graph.root
        assert root.node_type == "Document"
        assert root.parent is None
        assert len(root.children) > 0

    def test_root_id_matches_document_info(self, shannon_graph: BlazeGraph) -> None:
        root = shannon_graph.root
        assert root.id == shannon_graph.document_info.root_id

    def test_document_node_fields(self, shannon_graph: BlazeGraph) -> None:
        node = shannon_graph.nodes[0]
        assert isinstance(node, DocumentNode)
        assert isinstance(node.id, str)
        assert isinstance(node.node_type, str)
        assert isinstance(node.location, NodeLocation)
        assert isinstance(node.content, NodeContent)
        assert isinstance(node.token_count, int)
        assert isinstance(node.children, list)

    def test_semantic_location(self, shannon_graph: BlazeGraph) -> None:
        node = shannon_graph.nodes[0]
        sem = node.location.semantic
        assert isinstance(sem, SemanticLocation)
        assert isinstance(sem.path, str)
        assert isinstance(sem.depth, int)
        assert isinstance(sem.breadcrumbs, list)
        assert all(isinstance(b, str) for b in sem.breadcrumbs)

    def test_physical_location_present(self, shannon_graph: BlazeGraph) -> None:
        """At least some nodes should have physical locations (it's a PDF)."""
        nodes_with_phys = [
            n for n in shannon_graph.nodes if n.location.physical is not None
        ]
        assert len(nodes_with_phys) > 0

        phys = nodes_with_phys[0].location.physical
        assert isinstance(phys, PhysicalLocation)
        assert isinstance(phys.page, int)
        assert phys.page >= 1
        assert isinstance(phys.bounding_box, BoundingBox)
        assert isinstance(phys.bounding_box.x, float)
        assert isinstance(phys.bounding_box.y, float)
        assert isinstance(phys.bounding_box.width, float)
        assert isinstance(phys.bounding_box.height, float)

    def test_physical_location_null_for_root(self, shannon_graph: BlazeGraph) -> None:
        root = shannon_graph.root
        assert root.location.physical is None

    def test_sections_filter(self, shannon_graph: BlazeGraph) -> None:
        sections = shannon_graph.sections
        assert len(sections) > 0
        assert all(s.node_type == "Section" for s in sections)

    def test_paragraphs_filter(self, shannon_graph: BlazeGraph) -> None:
        paragraphs = shannon_graph.paragraphs
        assert len(paragraphs) > 0
        assert all(p.node_type == "Paragraph" for p in paragraphs)

    def test_get_node(self, shannon_graph: BlazeGraph) -> None:
        first = shannon_graph.nodes[0]
        fetched = shannon_graph.get_node(first.id)
        assert fetched is first

    def test_get_node_missing(self, shannon_graph: BlazeGraph) -> None:
        import pytest
        with pytest.raises(KeyError):
            shannon_graph.get_node("nonexistent-id")

    def test_nodes_by_page(self, shannon_graph: BlazeGraph) -> None:
        page1 = shannon_graph.nodes_by_page(1)
        assert len(page1) > 0
        for n in page1:
            assert n.location.physical is not None
            assert n.location.physical.page == 1

    def test_tree_navigation_parent(self, shannon_graph: BlazeGraph) -> None:
        # Get a non-root node
        non_root = [n for n in shannon_graph.nodes if n.parent is not None]
        assert len(non_root) > 0
        node = non_root[0]
        parent = node.get_parent(shannon_graph)
        assert parent is not None
        assert parent.id == node.parent

    def test_tree_navigation_children(self, shannon_graph: BlazeGraph) -> None:
        root = shannon_graph.root
        children = root.get_children(shannon_graph)
        assert len(children) == len(root.children)
        for child, child_id in zip(children, root.children):
            assert child.id == child_id

    def test_root_parent_is_none(self, shannon_graph: BlazeGraph) -> None:
        root = shannon_graph.root
        assert root.get_parent(shannon_graph) is None

    def test_document_metadata(self, shannon_graph: BlazeGraph) -> None:
        meta = shannon_graph.document_info.document_metadata
        assert isinstance(meta.page_count, int)
        assert meta.page_count > 0

    def test_document_analysis(self, shannon_graph: BlazeGraph) -> None:
        analysis = shannon_graph.document_info.document_analysis
        assert isinstance(analysis.font_size_counts, dict)
        assert isinstance(analysis.most_common_font_size, float)

    def test_structural_profile(self, shannon_graph: BlazeGraph) -> None:
        sp = shannon_graph.structural_profile
        assert sp.total_nodes > 0
        assert sp.total_tokens > 0
        assert isinstance(sp.document_type, str)
        assert isinstance(sp.flow_type, str)

    def test_structural_profile_distributions(self, shannon_graph: BlazeGraph) -> None:
        sp = shannon_graph.structural_profile
        assert sp.node_type_distribution is not None
        assert "Paragraph" in sp.node_type_distribution.counts
        assert sp.depth_distribution is not None
        assert sp.depth_distribution.max_depth > 0

    def test_to_dict_roundtrip(self, shannon_graph: BlazeGraph, shannon_raw: dict) -> None:
        assert shannon_graph.to_dict() is shannon_raw

    def test_to_json(self, shannon_graph: BlazeGraph) -> None:
        import json
        j = shannon_graph.to_json()
        parsed = json.loads(j)
        assert parsed["schema_version"] == "0.2.0"
        assert len(parsed["nodes"]) == 95


class TestNodeContent:
    """Verify node content text is accessible."""

    def test_content_text_accessible(self, shannon_graph: BlazeGraph) -> None:
        for node in shannon_graph.nodes[:5]:
            assert isinstance(node.content.text, str)
            assert len(node.content.text) > 0

    def test_section_has_meaningful_text(self, shannon_graph: BlazeGraph) -> None:
        sections = shannon_graph.sections
        assert any("Communication" in s.content.text for s in sections)

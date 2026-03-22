"""Typed data model for Blazegraph document graphs.

All dataclasses mirror the Rust types in ``blazegraph-core/src/types.rs``
(schema version 0.2.0). Designed for full IDE autocomplete.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


# ---------------------------------------------------------------------------
# Leaf types
# ---------------------------------------------------------------------------


@dataclass
class BoundingBox:
    """Physical bounding box on a PDF page (PDF points, origin top-left)."""

    x: float
    y: float
    width: float
    height: float

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "BoundingBox":
        return cls(x=d["x"], y=d["y"], width=d["width"], height=d["height"])


@dataclass
class SemanticLocation:
    """Where a node sits in the document tree."""

    path: str
    depth: int
    breadcrumbs: List[str]

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "SemanticLocation":
        return cls(path=d["path"], depth=d["depth"], breadcrumbs=list(d["breadcrumbs"]))


@dataclass
class PhysicalLocation:
    """Where a node appears on the physical page."""

    page: int
    bounding_box: BoundingBox

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "PhysicalLocation":
        return cls(
            page=d["page"],
            bounding_box=BoundingBox.from_dict(d["bounding_box"]),
        )


@dataclass
class NodeLocation:
    """Combined semantic + physical location."""

    semantic: SemanticLocation
    physical: Optional[PhysicalLocation]

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "NodeLocation":
        phys = d.get("physical")
        return cls(
            semantic=SemanticLocation.from_dict(d["semantic"]),
            physical=PhysicalLocation.from_dict(phys) if phys else None,
        )


@dataclass
class NodeContent:
    """Node text content."""

    text: str

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "NodeContent":
        return cls(text=d["text"])


# ---------------------------------------------------------------------------
# DocumentNode
# ---------------------------------------------------------------------------


@dataclass
class DocumentNode:
    """A single node in the document graph."""

    id: str
    node_type: str
    location: NodeLocation
    text_order: Optional[int]
    content: NodeContent
    token_count: int
    parent: Optional[str]
    children: List[str]

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "DocumentNode":
        return cls(
            id=d["id"],
            node_type=d["node_type"],
            location=NodeLocation.from_dict(d["location"]),
            text_order=d.get("text_order"),
            content=NodeContent.from_dict(d["content"]),
            token_count=d["token_count"],
            parent=d.get("parent"),
            children=list(d.get("children", [])),
        )

    # -- Tree navigation helpers --

    def get_parent(self, graph: "BlazeGraph") -> Optional["DocumentNode"]:
        """Return the parent node, or ``None`` for the root."""
        if self.parent is None:
            return None
        return graph.get_node(self.parent)

    def get_children(self, graph: "BlazeGraph") -> List["DocumentNode"]:
        """Return resolved child nodes in text order."""
        return [graph.get_node(cid) for cid in self.children]

    # -- Render --

    def render(
        self,
        graph: "BlazeGraph",
        *,
        breadcrumbs: bool = False,
        node_types: bool = False,
    ) -> str:
        """Render this node and all descendants as human-readable text.

        Args:
            graph: The parent ``BlazeGraph`` (needed for child lookup).
            breadcrumbs: Show breadcrumb trail on section/document nodes.
            node_types: Show ``[Type]`` prefix on each node.
        """
        parts: List[str] = []
        self._render_into(parts, graph, breadcrumbs=breadcrumbs, node_types=node_types)
        return "\n\n".join(parts)

    def _render_into(
        self,
        parts: List[str],
        graph: "BlazeGraph",
        *,
        breadcrumbs: bool,
        node_types: bool,
    ) -> None:
        """Recursively collect rendered text fragments."""
        header = self._render_header(breadcrumbs=breadcrumbs, node_types=node_types)
        if header:
            parts.append(header)

        for child in self.get_children(graph):
            child._render_into(parts, graph, breadcrumbs=breadcrumbs, node_types=node_types)

    def _render_header(self, *, breadcrumbs: bool, node_types: bool) -> str:
        """Build the header string for this single node."""
        is_structural = self.node_type in ("Section", "Document")
        use_breadcrumbs = breadcrumbs and is_structural

        if use_breadcrumbs and node_types:
            # [Section | crumb > crumb]
            trail = " > ".join(self.location.semantic.breadcrumbs)
            return f"[{self.node_type} | {trail}]"
        elif use_breadcrumbs:
            # [crumb > crumb]
            trail = " > ".join(self.location.semantic.breadcrumbs)
            return f"[{trail}]"
        elif node_types:
            # [Section] text
            return f"[{self.node_type}] {self.content.text}"
        else:
            # plain text
            return self.content.text


# ---------------------------------------------------------------------------
# DocumentInfo sub-types
# ---------------------------------------------------------------------------


@dataclass
class DocumentMetadata:
    """Metadata extracted from the PDF."""

    title: Optional[str] = None
    author: Optional[str] = None
    language: Optional[str] = None
    page_count: int = 0
    publisher: Optional[str] = None
    creator_tool: Optional[str] = None
    producer: Optional[str] = None
    pdf_version: Optional[str] = None
    created: Optional[str] = None
    modified: Optional[str] = None
    description: Optional[str] = None
    encrypted: Optional[bool] = None
    has_marked_content: Optional[bool] = None

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "DocumentMetadata":
        return cls(
            title=d.get("title"),
            author=d.get("author"),
            language=d.get("language"),
            page_count=d.get("page_count", 0),
            publisher=d.get("publisher"),
            creator_tool=d.get("creator_tool"),
            producer=d.get("producer"),
            pdf_version=d.get("pdf_version"),
            created=d.get("created"),
            modified=d.get("modified"),
            description=d.get("description"),
            encrypted=d.get("encrypted"),
            has_marked_content=d.get("has_marked_content"),
        )


@dataclass
class DocumentAnalysis:
    """Statistical analysis of the document's typographic structure."""

    font_size_counts: Dict[str, int] = field(default_factory=dict)
    font_family_counts: Dict[str, int] = field(default_factory=dict)
    bold_counts: List[int] = field(default_factory=list)
    italic_counts: List[int] = field(default_factory=list)
    most_common_font_size: float = 0.0
    most_common_font_family: str = ""
    all_font_sizes: List[float] = field(default_factory=list)

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "DocumentAnalysis":
        return cls(
            font_size_counts=dict(d.get("font_size_counts", {})),
            font_family_counts=dict(d.get("font_family_counts", {})),
            bold_counts=list(d.get("bold_counts", [])),
            italic_counts=list(d.get("italic_counts", [])),
            most_common_font_size=d.get("most_common_font_size", 0.0),
            most_common_font_family=d.get("most_common_font_family", ""),
            all_font_sizes=list(d.get("all_font_sizes", [])),
        )


@dataclass
class DocumentInfo:
    """Document-level metadata — information *about* the document."""

    root_id: str
    document_metadata: DocumentMetadata
    document_analysis: DocumentAnalysis

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "DocumentInfo":
        return cls(
            root_id=d["root_id"],
            document_metadata=DocumentMetadata.from_dict(d.get("document_metadata", {})),
            document_analysis=DocumentAnalysis.from_dict(d.get("document_analysis", {})),
        )


# ---------------------------------------------------------------------------
# StructuralProfile sub-types
# ---------------------------------------------------------------------------


@dataclass
class HistogramBin:
    """A single bin in a token distribution histogram."""

    range_start: int
    range_end: int
    count: int
    token_sum: int

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "HistogramBin":
        return cls(
            range_start=d["range_start"],
            range_end=d["range_end"],
            count=d["count"],
            token_sum=d["token_sum"],
        )


@dataclass
class TokenHistogram:
    """Token count distribution for a set of nodes."""

    bins: List[HistogramBin] = field(default_factory=list)
    total_count: int = 0
    total_tokens: int = 0
    mean: float = 0.0
    median: float = 0.0
    mode: Optional[int] = None
    variance: float = 0.0

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "TokenHistogram":
        return cls(
            bins=[HistogramBin.from_dict(b) for b in d.get("bins", [])],
            total_count=d.get("total_count", 0),
            total_tokens=d.get("total_tokens", 0),
            mean=d.get("mean", 0.0),
            median=d.get("median", 0.0),
            mode=d.get("mode"),
            variance=d.get("variance", 0.0),
        )


@dataclass
class TokenDistribution:
    """Per-type and overall token distributions."""

    by_node_type: Dict[str, TokenHistogram] = field(default_factory=dict)
    overall: Optional[TokenHistogram] = None

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "TokenDistribution":
        by_type = {
            k: TokenHistogram.from_dict(v) for k, v in d.get("by_node_type", {}).items()
        }
        overall_raw = d.get("overall")
        return cls(
            by_node_type=by_type,
            overall=TokenHistogram.from_dict(overall_raw) if overall_raw else None,
        )


@dataclass
class NodeTypeDistribution:
    """Node type counts and percentages."""

    counts: Dict[str, int] = field(default_factory=dict)
    percentages: Dict[str, float] = field(default_factory=dict)

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "NodeTypeDistribution":
        return cls(
            counts=dict(d.get("counts", {})),
            percentages=dict(d.get("percentages", {})),
        )


@dataclass
class DepthDistribution:
    """Tree depth statistics."""

    max_depth: int = 0
    depth_counts: Dict[str, int] = field(default_factory=dict)
    avg_depth: float = 0.0

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "DepthDistribution":
        return cls(
            max_depth=d.get("max_depth", 0),
            depth_counts=dict(d.get("depth_counts", {})),
            avg_depth=d.get("avg_depth", 0.0),
        )


@dataclass
class StructuralProfile:
    """Statistical properties of the document graph."""

    created_at: str = ""
    document_type: str = "Generic"
    flow_type: str = "Fixed"
    total_nodes: int = 0
    total_tokens: int = 0
    token_distribution: Optional[TokenDistribution] = None
    node_type_distribution: Optional[NodeTypeDistribution] = None
    depth_distribution: Optional[DepthDistribution] = None

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "StructuralProfile":
        td = d.get("token_distribution")
        ntd = d.get("node_type_distribution")
        dd = d.get("depth_distribution")
        return cls(
            created_at=d.get("created_at", ""),
            document_type=d.get("document_type", "Generic"),
            flow_type=d.get("flow_type", "Fixed"),
            total_nodes=d.get("total_nodes", 0),
            total_tokens=d.get("total_tokens", 0),
            token_distribution=TokenDistribution.from_dict(td) if td else None,
            node_type_distribution=NodeTypeDistribution.from_dict(ntd) if ntd else None,
            depth_distribution=DepthDistribution.from_dict(dd) if dd else None,
        )


# ---------------------------------------------------------------------------
# BlazeGraph — top-level return type
# ---------------------------------------------------------------------------


@dataclass
class BlazeGraph:
    """Top-level wrapper for a parsed document graph.

    This is the return type for :func:`blazegraphio.parse_pdf` and
    :func:`blazegraphio.parse_pdf_async`.
    """

    schema_version: str
    nodes: List[DocumentNode]
    document_info: DocumentInfo
    structural_profile: StructuralProfile
    _raw: Dict[str, Any] = field(default_factory=dict, repr=False)
    _index: Dict[str, DocumentNode] = field(default_factory=dict, repr=False)

    def __post_init__(self) -> None:
        # Build ID → node index for fast lookup
        if not self._index:
            self._index = {node.id: node for node in self.nodes}

    def __repr__(self) -> str:
        return f"<BlazeGraph: {len(self.nodes)} nodes, schema v{self.schema_version}>"

    # -- Filtered accessors --

    @property
    def sections(self) -> List[DocumentNode]:
        """All Section nodes."""
        return [n for n in self.nodes if n.node_type == "Section"]

    @property
    def paragraphs(self) -> List[DocumentNode]:
        """All Paragraph nodes."""
        return [n for n in self.nodes if n.node_type == "Paragraph"]

    @property
    def root(self) -> DocumentNode:
        """The Document root node."""
        return self._index[self.document_info.root_id]

    # -- Lookup helpers --

    def get_node(self, node_id: str) -> DocumentNode:
        """Look up a node by UUID.

        Raises:
            KeyError: If the node ID is not found.
        """
        return self._index[node_id]

    def nodes_by_page(self, page: int) -> List[DocumentNode]:
        """Return all nodes on a specific page number."""
        return [
            n
            for n in self.nodes
            if n.location.physical is not None and n.location.physical.page == page
        ]

    # -- Render --

    def render(
        self,
        *,
        breadcrumbs: bool = False,
        node_types: bool = False,
    ) -> str:
        """Render the full document as human-readable text."""
        return self.root.render(self, breadcrumbs=breadcrumbs, node_types=node_types)

    # -- Serialization --

    def to_dict(self) -> Dict[str, Any]:
        """Return the raw dictionary (the original JSON)."""
        return self._raw

    def to_json(self) -> str:
        """Return the graph as a JSON string."""
        import json

        return json.dumps(self._raw, indent=2)

    # -- Construction --

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "BlazeGraph":
        """Construct a ``BlazeGraph`` from a raw dictionary (parsed JSON).

        The dictionary should have the ``SortedDocumentGraph`` shape:
        ``schema_version``, ``nodes``, ``document_info``, ``structural_profile``.
        """
        nodes = [DocumentNode.from_dict(n) for n in d["nodes"]]
        return cls(
            schema_version=d["schema_version"],
            nodes=nodes,
            document_info=DocumentInfo.from_dict(d["document_info"]),
            structural_profile=StructuralProfile.from_dict(d["structural_profile"]),
            _raw=d,
        )

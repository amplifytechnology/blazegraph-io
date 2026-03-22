# Configuration Reference

Blazegraph's parsing pipeline is controlled by a YAML configuration. The default config works well for most documents. Create custom configs to tune thresholds for specific document categories.

**Source of truth:** [`config.rs`](../../../../blazegraph-io/blazegraph-core/src/config.rs)

---

## Pipeline

The parsing pipeline applies rules sequentially. Each rule receives the output of the previous rule.

```
PDF → Tika Extraction → [Rule 1] → [Rule 2] → ... → Graph Builder → bgraph.json
```

### Available Rules

| Rule | What it does | Default |
|------|-------------|---------|
| `SectionDetection` | Detects sections from font size, bold, and patterns. Assigns hierarchy levels. | Enabled |
| `PatternBasedSectionDetection` | Promotes elements to sections using regex patterns only (no font analysis). | Disabled |
| `SpatialClustering` | Merges adjacent text elements into coherent paragraphs. Two stages: paragraph merging, then spatial adjacency. | Enabled |
| `ListDetection` | Detects bullet and numbered lists. Two-phase: sequence detection, then content classification with validation. | Disabled in default config |
| `SizeEnforcer` | Splits oversized nodes at sentence boundaries. | Disabled in default config |
| `Validation` | Post-processing cleanup and validation. | Disabled in default config |

### Pipeline Configuration

```yaml
pipeline:
  rules:
    - name: "SectionDetection"
      enabled: true
    - name: "SpatialClustering"
      enabled: true
```

Rules execute in the order listed. The default pipeline runs SectionDetection first (to identify structural boundaries), then SpatialClustering (to merge text elements within those boundaries).

---

## Section Detection

Controls how Blazegraph identifies sections (headings) in the document.

### Font-Based Detection

Sections are detected by comparing each element's font size to the document's median body text size.

```yaml
section_and_hierarchy:
  large_header_threshold: 0.7     # 70% above median → large header
  medium_header_threshold: 0.5    # 50% above median → medium header
  small_header_threshold: 0.15    # 15% above median → small header
  min_header_size: 9.5            # Absolute minimum font size (points)
  use_bold_indicator: true        # Bold text can indicate headers
  bold_size_strict: false         # false = bold OR larger, true = bold AND larger
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `large_header_threshold` | float | 0.7 | % above median font size for top-level headers |
| `medium_header_threshold` | float | 0.5 | % above median for mid-level headers |
| `small_header_threshold` | float | 0.15 | % above median for lowest-level headers |
| `min_header_size` | float | 9.5 | Floor — nothing below this size is a header (points) |
| `use_bold_indicator` | bool | true | Consider bold formatting as a header signal |
| `bold_size_strict` | bool | false | `true`: bold AND larger required. `false`: bold OR larger. |

### Hierarchy

```yaml
section_and_hierarchy:
  max_depth: 6                    # Maximum section nesting depth
  font_size_tolerance: 0.1        # Tolerance for same-level font sizes
  enforce_max_depth: true         # Cap depth at max_depth
  starting_section_level: 1       # Starting level for first detected section
```

### Pattern-Based Detection

Regex patterns that promote elements to sections (in addition to font-based detection).

```yaml
section_and_hierarchy:
  pattern_detection:
    enabled: true
    respect_font_constraints: true   # Only promote if font size is also above threshold
    patterns:
      - "^[A-Z][A-Z\\s]{2,}$"                        # ALL CAPS (min 3 chars)
      - "^\\d+\\.\\s+[A-Z][a-z]{3,}"                 # "1. Title" format
      - "^(Chapter|Section|Part|Article)\\s+\\d+"      # Explicit structural words
      - "^[A-Z][a-z]{2,}(?:\\s+[A-Z][a-z]{2,})*:$"   # "Title Case:" with colon
```

---

## Spatial Clustering

Controls how raw text elements are merged into coherent paragraphs.

The default pipeline runs two stages:
1. **Paragraph merging** — combines segments that share the same paragraph number (from Tika's detection)
2. **Spatial adjacency** — groups physically close elements based on gap analysis

```yaml
spatial_clustering:
  enabled: true
  enable_paragraph_merging: true     # Stage 1: merge by paragraph number
  enable_spatial_adjacency: true     # Stage 2: merge by physical proximity
```

### Spatial Parameters

```yaml
spatial_clustering:
  min_line_height: 8.0                          # Minimum line height (points)
  vertical_gap_threshold_multiplier: 10.0       # Gap = multiplier × line_height
  horizontal_alignment_tolerance: 15.0          # X-tolerance for same column (points)
  line_grouping_tolerance: 0.5                  # Line grouping tolerance
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `vertical_gap_threshold_multiplier` | float | 10.0 | **Key tuning parameter.** Higher = less sensitive = fewer, larger paragraphs. Lower = more sensitive = more, smaller segments. Range: 0.6 (legal) to 10.0+ (conservative). |
| `horizontal_alignment_tolerance` | float | 15.0 | How far apart (in points) elements can be horizontally and still be grouped. |
| `min_line_height` | float | 8.0 | Minimum line height for gap calculations. |
| `line_grouping_tolerance` | float | 0.5 | Tolerance for grouping elements on the same line (as fraction of line height). |

### Segment Size Limits

Control the minimum and maximum character count for clustered segments.

```yaml
spatial_clustering:
  sections:
    min_segment_size: 20            # Short section titles allowed
    max_segment_size: 300           # Keep section headers concise
  paragraphs:
    min_segment_size: 50            # Minimum paragraph size
    max_segment_size: 3000          # Maximum paragraph size
```

---

## List Detection

Two-phase detection of bullet and numbered lists.

```yaml
list_detection:
  enabled: true

  # Phase 1: Sequence detection
  sequence_lookahead_elements: 10   # How far ahead to look for next marker
  sequence_boundary_extension: 3    # Elements past last marker to include

  # Phase 2: Spatial validation
  y_tolerance: 10.0                 # Y-coordinate tolerance (points)
  last_item_boundary_gap: 80.0      # Gap threshold for sequence end (points)

  # Recognized patterns
  bullet_patterns: ["•", "·", "●", "■", "▪", "◦", "-", "*", "→"]
  numbered_patterns:
    - "^\\d+\\."         # 1., 2., 3.
    - "^\\d+\\)"         # 1), 2), 3)
    - "^\\(\\d+\\)"      # (1), (2), (3)
    - "^[a-z]\\."        # a., b., c.
    - "^[A-Z]\\."        # A., B., C.
    - "^[ivx]+\\."       # i., ii., iii.
    - "^[IVX]+\\."       # I., II., III.

  # Validation
  validation:
    enabled: true
    minimum_size_check: true              # Lists must have >1 item
    first_item_validation: true           # Numbered lists must start with 1
    sequential_numbering_check: true      # Numbers must be sequential
    mathematical_context_check: true      # Avoid false positives in math
    hyphen_context_check: true            # Distinguish bullets from hyphens
```

---

## Size Enforcer

Splits nodes that exceed a character limit, respecting sentence boundaries.

```yaml
size_enforcer:
  enabled: true
  max_size: 400                 # Maximum characters per node
  size_unit: "characters"       # "characters", "words", or "bytes"
  preserve_sentences: true      # Split at sentence boundaries
  min_split_size_ratio: 0.25    # Minimum chunk = 25% of max_size
  recursive: true               # Keep splitting until all nodes comply
  max_iterations: 10            # Safety limit
```

Useful for RAG pipelines where chunk size matters. Set `max_size` to your embedding model's sweet spot.

---

## Using Configs

Pass a YAML config file to the CLI:

```bash
blazegraph-io parse document.pdf -c my-config.yaml -o bgraph.json
```

Build one config per document category (e.g., all your legal contracts, or all academic papers from a specific journal) and reuse it across that group. The default config works well for general-purpose parsing — custom configs are for when you need to tune specific thresholds.

---

## Tuning Guide

### Too many sections detected?

Raise the header thresholds:

```yaml
section_and_hierarchy:
  large_header_threshold: 0.8     # Stricter (was 0.7)
  small_header_threshold: 0.2     # Stricter (was 0.15)
  bold_size_strict: true          # Require BOTH bold AND larger
```

### Paragraphs too fragmented?

Increase the gap threshold (less sensitive to vertical spacing):

```yaml
spatial_clustering:
  vertical_gap_threshold_multiplier: 2.0   # Was 10.0 — even more conservative
  paragraphs:
    max_segment_size: 5000                  # Allow bigger paragraphs
```

### Paragraphs too large?

Decrease the gap threshold (more sensitive) or enable the size enforcer:

```yaml
spatial_clustering:
  vertical_gap_threshold_multiplier: 0.6   # Very sensitive to gaps

# Or enforce max size:
size_enforcer:
  enabled: true
  max_size: 500
```

### Need list detection?

Enable it in the pipeline:

```yaml
pipeline:
  rules:
    - name: "SectionDetection"
      enabled: true
    - name: "ListDetection"
      enabled: true
    - name: "SpatialClustering"
      enabled: true
```

---

## Minimal Parse Mode

For debugging or when you want raw extraction without semantic processing:

```yaml
minimal_parse: true
```

This bypasses all rules and converts each Tika text element directly to a Paragraph node. Useful for understanding what the PDF extractor sees before Blazegraph applies its rules.

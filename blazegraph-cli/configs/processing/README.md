# Blazegraph Configuration System

This directory contains all configuration files for the Blazegraph document parser. This README serves as the comprehensive documentation for all configuration parameters and their usage.

## Table of Contents
1. [Directory Structure](#directory-structure)
2. [Configuration Parameters](#configuration-parameters)
3. [Document Type Variants](#document-type-variants)
4. [Pipeline Configuration](#pipeline-configuration)
5. [Usage Guide](#usage-guide)
6. [Configuration Management](#configuration-management)

## Directory Structure

```
configs/
├── config.yaml         # Default configuration (active development config)
├── minimal-parse.yaml  # Minimal processing pipeline for debugging
├── templates/          # Document-type-specific base templates (empty)
├── optimized/          # Production-ready optimized configurations (empty)
├── archived/           # Historical test variants and experiments (empty)
└── README.md           # This comprehensive documentation file
```

**Note**: The templates/, optimized/, and archived/ directories exist but are currently empty. This represents an opportunity for better configuration organization.

## Configuration Parameters

### Core Configuration Structure

```yaml
document_type: Generic|AcademicPaper|LegalContract|TechnicalManual|BusinessReport
pipeline:
  rules: [list of rule configurations]
section_and_hierarchy: [section detection parameters]
spatial_clustering: [spatial analysis parameters]
section_patterns: [list of section keywords]
include_raw_tika: true|false
```

### Pipeline Configuration

**Status: ACTIVE** - Used in rule execution engine

Controls which parsing rules run and in what order:

```yaml
pipeline:
  rules:
    - name: "SectionDetection"              # Font-based section detection
      enabled: true
    - name: "PatternBasedSectionDetection"  # Regex pattern-based detection  
      enabled: false
    - name: "SpatialClustering"             # Spatial proximity clustering
      enabled: true
    - name: "MinimalParse"                  # Minimal processing (debug)
      enabled: false
    - name: "Validation"                    # Post-processing validation
      enabled: true
```

**Available Rules:**
- `SectionDetection`: Font size and style-based section detection
- `PatternBasedSectionDetection`: Regex pattern-based section promotion
- `SpatialClustering`: Spatial proximity-based element clustering  
- `MinimalParse`: Converts each Tika element to paragraph (debugging)
- `Validation`: Post-processing validation and cleanup

### Section and Hierarchy Configuration

**Status: ACTIVE** - Core parameters for section detection

```yaml
section_and_hierarchy:
  # Font size analysis (percentages above median)
  large_header_threshold: 0.7    # 70% above median = large header
  medium_header_threshold: 0.3   # 30% above median = medium header  
  small_header_threshold: 0.1    # 10% above median = small header
  min_header_size: 8.5           # Absolute minimum font size for headers
  use_bold_indicator: true       # Consider bold text as header indicator
  bold_size_strict: true         # true = bold AND larger than content, false = bold OR larger
  
  # Hierarchy depth control
  max_depth: 5                   # Maximum section nesting depth
  font_size_tolerance: 0.1       # Font size tolerance for same level (points)
  enforce_max_depth: true        # Whether to enforce max_depth limit
  starting_section_level: 1      # Starting level for first section
  
  # Pattern-based detection
  pattern_detection:
    enabled: true                # Enable regex pattern matching
    respect_font_constraints: true  # Respect font size even with pattern match
    patterns:                    # Regex patterns for section headers
      - "^[A-Z][A-Z\\s]{2,}$"                   # ALL CAPS (min 3 chars)
      - "^\\d+\\.\\s+[A-Z][a-z]{3,}"           # "1. Title" format
      - "^(Chapter|Section|Part|Article)\\s+\\d+"  # Explicit structural words
      - "^[A-Z][a-z]{2,}(?:\\s+[A-Z][a-z]{2,})*:$" # "Title Case:" with colon
```

### Spatial Clustering Configuration  

**Status: ACTIVE** - Used in spatial analysis

```yaml
spatial_clustering:
  enabled: true                           # Enable spatial clustering
  min_line_height: 8.0                   # Minimum line height (points)
  vertical_gap_threshold_multiplier: 0.8  # Gap = multiplier × line_height for breaks
  horizontal_alignment_tolerance: 10.0    # X-coordinate tolerance (points)
  line_grouping_tolerance: 0.3           # Line grouping tolerance (% of line height)
  
  # Element-specific clustering
  sections:
    min_segment_size: 20    # Minimum characters for section titles
    max_segment_size: 300   # Maximum characters for section titles
  paragraphs:
    min_segment_size: 100   # Minimum characters for paragraphs
    max_segment_size: 8000  # Maximum characters for paragraphs
```

**Key Parameter Explanation:**
- `vertical_gap_threshold_multiplier`: Higher values = less sensitive = fewer segments
- Values range from 0.6 (very sensitive) to 10.0+ (very conservative)

### Content Classification Parameters

**Status: ACTIVE** - Used in section detection

```yaml
section_patterns:           # Keywords that indicate section headers  
  - "chapter"
  - "section" 
  - "part"
  - "overview"
  - "summary"
  - "background"
  - "principles"
  - "approach"
```

### Global Options

```yaml
include_raw_tika: false    # ACTIVE - Include raw Tika output in graph metadata
```

## Document Type Variants

The system supports different document types with specialized configurations:

### Generic (Default)
- **Use case**: General documents, sample PDFs
- **Characteristics**: Balanced thresholds, moderate depth limits
- **Spatial clustering**: `vertical_gap_threshold_multiplier: 0.8`

### AcademicPaper  
- **Use case**: Research papers, academic documents
- **Characteristics**: More conservative section detection, tighter alignment
- **Key differences**:
  - Higher header thresholds (0.8/0.4/0.15 vs 0.7/0.3/0.1)
  - More conservative spatial clustering (`multiplier: 1.2`)
  - Tighter alignment tolerance (8.0 vs 10.0 points)
  - Larger paragraph segments (200-12000 vs 100-8000 chars)

### LegalContract
- **Use case**: Legal documents, contracts, agreements  
- **Characteristics**: Very sensitive to small gaps, allows indented clauses
- **Key differences**:
  - Very sensitive spatial clustering (`multiplier: 0.6`)
  - Larger horizontal tolerance (12.0 points) for indented clauses  
  - Smaller paragraph segments (50-5000 chars) for digestible clauses
  - Very tight line grouping tolerance (0.2 vs 0.3)

## Pipeline Configuration

The parsing process follows a sequential pipeline architecture:

1. **Document Classification**: Determines document type
2. **Config Selection**: Chooses appropriate configuration  
3. **Rule Execution**: Applies enabled rules in sequence
4. **Output Generation**: Creates document graph

### Rule Execution Order

Rules process elements sequentially, with each rule receiving the output of the previous rule:

```
Text Elements → Rule 1 → Rule 2 → Rule 3 → Final Parsed Elements
```

**Typical Pipeline:**
1. `SectionDetection`: Identifies sections based on font size/style
2. `SpatialClustering`: Groups spatially related content
3. `Validation`: Applies post-processing validation

## Usage Guide

### Basic Usage

```bash
# Use default configuration
make run-bg

# Use specific configuration file
make run-bg CONFIG=configs/minimal-parse.yaml

# Include raw Tika output for debugging
make run-with-tika CONFIG=configs/config.yaml
```


### Configuration Testing

```bash
# Test different spatial clustering sensitivity
# Edit vertical_gap_threshold_multiplier: 0.6 (sensitive) to 2.0 (conservative)
make run-bg CONFIG=test-config.yaml

# Compare outputs in timestamped directories
ls outputs/
```

## Configuration Management

### Parameter Usage Status

**ACTIVE Parameters** (22 total):
- All `pipeline` parameters
- All `section_and_hierarchy` parameters  
- All `spatial_clustering` parameters
- `section_patterns` (used in section detection)
- `include_raw_tika` (used in output generation)

**UNUSED Parameters** (0 total):
- None remaining - all dead parameters have been removed

### Adding New Parameters

1. **Define in config.rs**: Add to appropriate struct with `#[serde(default)]` 
2. **Add default function**: Create `fn default_param_name() -> Type`
3. **Update Default impl**: Add to `impl Default` blocks
4. **Use in rules**: Reference in rule implementation code
5. **Test thoroughly**: Verify parameter affects behavior correctly

### Removing Parameters

1. **Check usage**: Search codebase for parameter references
2. **Update rules**: Remove usage from rule implementations  
3. **Update structs**: Remove from config structs
4. **Update defaults**: Remove from default implementations
5. **Update docs**: Remove from documentation

### Configuration Validation

**Current State**: No systematic validation exists

**Recommendations**:
- Add parameter range validation (e.g., thresholds 0.0-1.0)
- Validate pipeline rule names against available rules
- Check regex patterns compile correctly
- Validate numeric parameters are positive where required

### File Organization Strategy

**Current Organization**: Flat structure with main configs in root
**Recommended Improvements**:

```
configs/
├── config.yaml                    # Current active config
├── templates/
│   ├── generic-balanced.yaml      # Balanced generic template
│   ├── generic-aggressive.yaml    # High-sensitivity template  
│   ├── generic-conservative.yaml  # Low-sensitivity template
│   ├── academic-paper.yaml        # Academic document template
│   └── legal-contract.yaml        # Legal document template
├── optimized/
│   ├── production-generic.yaml    # Production-tested generic config
│   └── sample3-tuned.yaml        # Document-specific optimization
└── archived/
    ├── experiments/               # Experimental configurations
    └── deprecated/               # Deprecated configurations
```

### Best Practices

1. **Version Control**: Always commit config changes with descriptive messages
2. **Testing**: Test config changes with multiple document types
3. **Documentation**: Update this README when adding/removing parameters  
4. **Archival**: Move experimental configs to archived/ when complete
5. **Validation**: Use output comparison to validate config changes
6. **Naming**: Use descriptive names indicating purpose/document type

### Configuration Impact Analysis

Before changing configuration parameters:

1. **Identify affected rules**: Which rules use this parameter?
2. **Test scope**: Which document types will be affected?
3. **Backup current**: Copy current config before changes
4. **Compare outputs**: Run before/after comparisons
5. **Document changes**: Record parameter changes and rationale

### Migration Strategy

For config structure changes:

1. **Maintain backward compatibility**: Support old parameter names initially
2. **Deprecation warnings**: Log warnings for deprecated parameters
3. **Migration script**: Provide automated config migration if needed
4. **Documentation**: Clearly document breaking changes
5. **Timeline**: Allow sufficient time for migration

This comprehensive documentation should be updated whenever configuration parameters are added, removed, or their usage changes.
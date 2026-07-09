use anyhow::{anyhow, Result};
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use std::path::Path;

// Import from blazegraph-io-core
use blazegraph_io_core::{
    CacheDefaults, CachePoint, DocumentGraph, DocumentProcessor, FreshFrom, ParseProvenance,
    ParsingConfig, PipelineStages,
};

/// Default config embedded at compile time — guarantees every install has working defaults.
/// Without this, `cargo install` users get raw parse output (3000+ nodes, 0 sections).
const DEFAULT_CONFIG_YAML: &str = include_str!("../configs/processing/config.yaml");

// Import CLI utilities
#[cfg(feature = "jni-backend")]
use blazegraph_io::JreManager;

// =========================================================================
// CLI surface
// =========================================================================
//
// B5 (2026-05-10) introduced an explicit subcommand surface — `parse`
// and `strip` — alongside markdown input/output support. There is no
// flag-only fallthrough mode: prior to B5 the CLI was a bare-args
// invocation (`blazegraph -i foo.pdf`), but the design dialogue locked
// in subcommands for clarity (no real users to preserve). The README
// and example invocations use `blazegraph parse ...` and
// `blazegraph strip ...` as the canonical forms.

#[derive(Parser)]
#[command(name = "blazegraph")]
#[command(about = "A semantic document graph parser with configurable rules")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse a document into a semantic graph (or emit it back to
    /// markdown).
    ///
    /// Input formats: `.pdf` (PDF channel), `.docx` (OOXML channel),
    /// and `.bgraph.md` / `.md` (markdown channel — bgraph.md
    /// round-trip artifact or generic markdown, lib auto-detects).
    /// Format detection is by extension first; content-sniff for
    /// unknown extensions.
    Parse(ParseArgs),

    /// Strip bgraph fences from a bgraph.md file.
    ///
    /// `body-with-frontmatter` (default) strips every bgraph fence and
    /// lifts the doc-level `bgraph` block to YAML frontmatter at the
    /// top of the output — produces docling-comparable plain markdown
    /// with provenance preserved. `body-only` strips every bgraph
    /// fence and drops metadata entirely (Unstructured-equivalent).
    /// `--node-types` is an orthogonal filter that removes specified
    /// element types entirely via the spec's structural rule;
    /// composes with `--mode`.
    Strip(StripArgs),
}

#[derive(ClapArgs)]
struct ParseArgs {
    /// Path to the input file (PDF, .docx, .bgraph.md, or .md).
    #[arg(short, long, default_value = "../sample_pdfs/sample3.pdf")]
    input: String,

    /// Path to custom config file (YAML format). PDF channel only.
    #[arg(short, long)]
    config: Option<String>,

    /// Output format: `graph` (default JSON), `sequential`, `flat`,
    /// `markdown`, or `bgraph-md`.
    ///
    /// - `markdown` (B6) emits plain markdown via the generic-markdown
    ///   emitter. Inverse of generic-markdown input.
    /// - `bgraph-md` (was `markdown` in B5) emits the bgraph.md
    ///   round-trip artifact with embedded fences. Inverse of
    ///   bgraph.md input.
    #[arg(short = 'f', long, default_value = "graph")]
    output_format: String,

    /// Show available config options and exit.
    #[arg(long)]
    show_configs: bool,

    /// Output file path (if not specified, auto-generated based on input).
    ///
    /// Auto-generated suffix is `.bgraph.md` for `-f markdown`, otherwise
    /// `_blazegraph.json`.
    #[arg(short, long)]
    output: Option<String>,

    /// Enable minimal parse mode (bypass all rule processing). PDF channel only.
    #[arg(long)]
    minimal_parse: bool,

    /// Path to JRE directory (for JNI backend). PDF channel only.
    /// If not specified, JRE will be auto-downloaded on first use.
    #[arg(long)]
    jre_path: Option<String>,

    /// Path to Tika JAR file (for JNI backend). PDF channel only.
    /// If not specified, uses bundled JAR.
    #[arg(long)]
    jar_path: Option<String>,

    /// Enable detailed profiling of all pipeline steps. PDF channel only.
    #[arg(long)]
    profile: bool,

    /// Include `style` on every per-element fence in the emitted bgraph.md
    /// (verbatim Tika projection — `foreground_color`, `background_color`,
    /// `font_family`, `font_size`, `is_bold`, `is_italic`, `font_class`).
    /// CR-59 reverted the default to opt-in: by default the wire-format
    /// emitter omits `style` (the in-memory `node.style_info` is still
    /// populated for library consumers). Pass this flag to round-trip a
    /// PDF-source graph with style preserved in the emitted bgraph.md.
    /// PDF channel only.
    #[arg(long)]
    include_style_info: bool,

    /// Dump all intermediate pipeline stage outputs to a directory.
    /// Captures: XHTML, TextElements, ParsedElements, and final Graph as separate files.
    /// PDF channel only.
    #[arg(long)]
    dump_stages: bool,

    /// Directory for stage dump output (default: {cache_dir}/debug).
    #[arg(long)]
    stages_dir: Option<String>,

    // =========================================================================
    // Cache control (CR-11)
    // =========================================================================
    /// Override cache directory location.
    /// Default: ~/.local/share/blazegraph/cache/
    /// Also configurable via BLAZEGRAPH_CACHE_DIR env var.
    #[arg(long)]
    cache_dir: Option<String>,

    /// Reprocess from a specific cache point, ignoring cached results downstream.
    /// Cascades: --fresh-from c1 also invalidates c2 and c3.
    /// Values: c0 (re-read PDF), c1 (re-extract XHTML), c2 (reparse elements), c3 (rebuild graph)
    #[arg(long)]
    fresh_from: Option<String>,

    /// Clear cached files from the specified cache point (cascading) and exit.
    /// Values: c0, c1, c2, c3, all
    #[arg(long)]
    clear_cache: Option<String>,

    /// Alias for --fresh-from c0 (reprocess everything from scratch).
    #[arg(long)]
    skip_cache: bool,

    // =========================================================================
    // Markdown-input flags (B5)
    // =========================================================================
    /// Accept hash-drifted bgraph.md input. When the recomputed
    /// `graph_sha256` does not match the value embedded in the
    /// doc-level block, return a derivative graph instead of erroring.
    /// Only meaningful when the input is markdown.
    #[arg(long)]
    accept_drift: bool,
}

#[derive(ClapArgs)]
struct StripArgs {
    /// Path to the bgraph.md file to strip. Source file is never modified.
    #[arg(short, long)]
    input: String,

    /// Output file path. If omitted, stripped content is written to stdout.
    #[arg(short, long)]
    output: Option<String>,

    /// Strip mode. Default: `body-with-frontmatter` — strip every
    /// fence, lift doc-level metadata to YAML frontmatter. Produces
    /// docling-comparable plain markdown with provenance preserved.
    ///
    /// Alternative:
    /// - `body-only`: strip every fence, drop metadata entirely.
    #[arg(long, value_enum, default_value_t = CliStripMode::BodyWithFrontmatter)]
    mode: CliStripMode,

    /// Comma-separated list of element types to strip entirely (body +
    /// fence) via the spec's structural rule. E.g.,
    /// `--node-types header,footer,margin` for RAG-clean output
    /// without running noise. Orthogonal to `--mode`: composes with
    /// the default and with any explicit mode (filter pass first,
    /// then mode pass).
    ///
    /// Valid types (current v2.0.0 set): header, footer, margin,
    /// section, paragraph, codeblock, list, blockquote, table,
    /// bookmarks. Unknown types are rejected with a list of valid
    /// values. `bgraph` (the doc-level fence) cannot be a node-type
    /// target — use `--mode body-only` to drop metadata instead.
    #[arg(long, value_delimiter = ',', value_parser = parse_node_type)]
    node_types: Vec<String>,
}

/// Valid v2.0.0 per-element fence tags (un-prefixed). Used for
/// CLI-level validation of `--node-types`. Mirrors the dashed-info-
/// string discipline in
/// `blazegraph_io_core::preprocessors::md::bgraph_md::bgraph_fence_open_tag`.
const VALID_NODE_TYPES: &[&str] = &[
    "bookmarks",
    "section",
    "paragraph",
    "header",
    "footer",
    "margin",
    "codeblock",
    "list",
    "blockquote",
    "table",
];

/// clap value-parser for `--node-types`. Rejects unknown tags and the
/// bare `bgraph` doc-level tag at the CLI layer with a hint to use
/// `--mode body-only` instead.
fn parse_node_type(s: &str) -> std::result::Result<String, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty node type".to_string());
    }
    if s == "bgraph" {
        return Err(
            "`bgraph` (the doc-level fence) cannot be a --node-types target; \
             use `--mode body-only` to drop metadata entirely"
                .to_string(),
        );
    }
    if VALID_NODE_TYPES.contains(&s) {
        Ok(s.to_string())
    } else {
        Err(format!(
            "unknown node type `{s}`; valid types: {}",
            VALID_NODE_TYPES.join(", ")
        ))
    }
}

/// CLI mirror of [`blazegraph_io_core::preprocessors::md::StripMode`]
/// (the two exposed-as-`--mode` variants — `NodeTypes` is reached via
/// the orthogonal `--node-types` flag, not as a `--mode` value).
///
/// Held separately so the CLI surface can use clap's `ValueEnum`
/// derive without imposing it on the lib type.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum CliStripMode {
    /// Strip every bgraph fence and lift the doc-level block to YAML
    /// frontmatter. Produces docling-comparable plain markdown.
    BodyWithFrontmatter,
    /// Remove all bgraph fences (Unstructured-equivalent body output).
    BodyOnly,
}

impl From<CliStripMode> for blazegraph_io_core::preprocessors::md::StripMode {
    fn from(m: CliStripMode) -> Self {
        use blazegraph_io_core::preprocessors::md::StripMode as Core;
        match m {
            CliStripMode::BodyWithFrontmatter => Core::BodyWithFrontmatter,
            CliStripMode::BodyOnly => Core::BodyOnly,
        }
    }
}

// =========================================================================
// Entry point
// =========================================================================

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Parse(args) => run_parse(args),
        Command::Strip(args) => run_strip(args),
    }
}

// =========================================================================
// `parse` subcommand
// =========================================================================

fn run_parse(args: ParseArgs) -> Result<()> {
    println!("🦀 Blazegraph Document Parser");

    if args.show_configs {
        show_help();
        return Ok(());
    }

    // Resolve cache directory: CLI flag > env var > default
    let cache_dir = resolve_cache_dir(&args)?;
    println!("📁 Cache: {}", cache_dir);

    // Handle --clear-cache (early exit, no processing)
    if let Some(ref clear_str) = args.clear_cache {
        let storage = blazegraph_io_core::storage::FileStorage::new(&cache_dir)?;
        let from_point = CachePoint::from_str_with_all(clear_str)?;
        let label = from_point
            .map(|p| format!("{} (cascading)", p))
            .unwrap_or_else(|| "all".to_string());
        println!("🗑️  Clearing cache from {}...", label);
        let result =
            <blazegraph_io_core::storage::FileStorage as blazegraph_io_core::storage::DocumentStorage>::clear_cache(
                &storage,
                from_point,
            )?;
        for (point, count) in &result.deleted {
            println!("   Deleted: {} files from {}/", count, point.dir_name());
        }
        println!("✅ Cache cleared.");
        return Ok(());
    }

    // Check if input file exists
    if !Path::new(&args.input).exists() {
        println!("⚠️  Input file not found at: {}", args.input);
        println!("   Please check the file path.");
        return Ok(());
    }

    // Branch on input format: markdown vs PDF.
    //
    // The markdown channel bypasses the PDF pipeline (no JRE / Tika /
    // rule engine). We detect by extension first, content-sniff
    // fallback for unknown extensions (see `detect_input_format`).
    //
    // Schema 0.7.0+ (B6): both bgraph.md and generic markdown flow
    // through the unified `parse_markdown` dispatcher. Routing is
    // the lib's job — we just pass the bytes.
    match detect_input_format(Path::new(&args.input))? {
        InputFormat::Markdown { content } => run_parse_markdown(args, content),
        InputFormat::Pdf => run_parse_pdf(args, cache_dir),
        InputFormat::Docx => run_parse_docx(args),
        InputFormat::Unknown => Err(anyhow!(
            "❌ Input format not recognized: {}\n\
             \n\
             Supported formats:\n\
             \t.pdf            — PDF documents (full parsing pipeline)\n\
             \t.docx           — Word documents (OOXML channel)\n\
             \t.bgraph.md      — Blazegraph markdown round-trip artifact\n\
             \t.md, .markdown  — Generic markdown\n",
            args.input
        )),
    }
}

// =========================================================================
// PDF channel (existing behavior, unchanged except for being inside
// `run_parse_pdf`)
// =========================================================================

fn run_parse_pdf(args: ParseArgs, cache_dir: String) -> Result<()> {
    // Create processor with resolved cache dir
    let mut processor = create_processor(&args, &cache_dir)?;

    // Load config: user-specified file > embedded default > ParsingConfig::default()
    let mut config = if let Some(config_path) = &args.config {
        let c = ParsingConfig::load_with_fallback(Some(config_path));
        println!("📋 Loaded config from: {}", config_path);
        c
    } else {
        match serde_yaml::from_str::<ParsingConfig>(DEFAULT_CONFIG_YAML) {
            Ok(c) => {
                println!("📋 Using built-in default config");
                c
            }
            Err(e) => {
                eprintln!("⚠️  Failed to parse embedded config: {e}, using fallback defaults");
                ParsingConfig::default()
            }
        }
    };

    // Apply CLI overrides to config
    if args.minimal_parse {
        config.minimal_parse = true;
    }

    // Resolve fresh-from: --skip-cache takes precedence
    let fresh_from = if args.skip_cache {
        FreshFrom::C0
    } else if let Some(ref s) = args.fresh_from {
        FreshFrom::parse(s)?
    } else {
        FreshFrom::None
    };

    let cache_defaults = CacheDefaults::default();

    println!("📄 Processing: {}", args.input);

    // Stage dump mode: capture and save all intermediates
    if args.dump_stages {
        let stages_dir = args
            .stages_dir
            .clone()
            .unwrap_or_else(|| format!("{}/debug", cache_dir));
        println!("\n🔬 Pipeline stage dump mode");
        match processor.process_document_capture_stages(&args.input, &config) {
            Ok(stages) => {
                save_stages(&stages, &stages_dir)?;
                println!("\n✅ All stages dumped to: {}", stages_dir);
            }
            Err(e) => {
                eprintln!("❌ Stage dump failed: {e}");
                std::process::exit(1);
            }
        }
        #[cfg(feature = "jni-backend")]
        std::process::exit(0);
        #[cfg(not(feature = "jni-backend"))]
        return Ok(());
    }

    // Process the document with cache point awareness
    match processor.process_document_with_cache(
        &args.input,
        &config,
        fresh_from,
        &cache_defaults,
        args.profile,
    ) {
        Ok((graph, provenance)) => {
            println!("✅ Successfully processed document");
            println!("📊 Graph: {} nodes", graph.nodes.len());

            // CR-59 (v2.1.0+): style emission is opt-in. The pipeline
            // always populates `node.style_info` for library consumers;
            // only the bgraph.md serializer gates emission, threaded
            // through `save_graph` → `EmitOptions::include_style_info`.
            // Block A: provenance rides beside the graph as a value.
            let output_path = resolve_output_path(&args);
            save_graph(
                &graph,
                &provenance,
                &output_path,
                &args.output_format,
                args.include_style_info,
            )?;

            // Fast exit - skip JVM shutdown sequence
            #[cfg(feature = "jni-backend")]
            std::process::exit(0);
            #[cfg(not(feature = "jni-backend"))]
            Ok(())
        }
        Err(e) => {
            eprintln!("❌ Processing failed: {e}");
            std::process::exit(1);
        }
    }
}

// =========================================================================
// Markdown channel (B5)
// =========================================================================

fn run_parse_markdown(args: ParseArgs, content: String) -> Result<()> {
    use blazegraph_io_core::preprocessors::md::{
        is_bgraph_md, parse_markdown, ParseError, ParseIdentity, ParseOptions,
    };

    let is_bgraph = is_bgraph_md(&content);
    if is_bgraph {
        println!("📄 Parsing bgraph.md: {}", args.input);
    } else {
        println!("📄 Parsing generic markdown: {}", args.input);
    }

    let opts = ParseOptions {
        accept_drift: args.accept_drift,
    };
    let result = match parse_markdown(&content, opts) {
        Ok(r) => r,
        Err(ParseError::HashMismatch {
            original,
            recomputed,
        }) => {
            eprintln!(
                "\n❌ bgraph.md graph_sha256 mismatch.\n\
                 \toriginal:   {original}\n\
                 \trecomputed: {recomputed}\n\
                 \n\
                 This means the bgraph.md has been edited since emission.\n\
                 To accept the drifted content as a new derivative graph,\n\
                 re-run with --accept-drift.\n"
            );
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("\n❌ markdown parse failed: {e}\n");
            std::process::exit(1);
        }
    };

    match result.identity {
        ParseIdentity::Verified => {
            println!("✅ Round-trip identity verified (graph_sha256 matches).");
        }
        ParseIdentity::Derivative {
            original_sha256,
            recomputed_sha256,
        } => {
            eprintln!(
                "⚠️  Graph reconstructed from drifted bgraph.md (--accept-drift):\n\
                 \toriginal graph_sha256:   {original_sha256}\n\
                 \trecomputed graph_sha256: {recomputed_sha256}\n\
                 \tThe reconstructed graph is a derivative, not an identity round-trip."
            );
        }
    }

    emit_parsed_graph(&args, result.graph, result.provenance)
}

// =========================================================================
// DOCX channel (C4)
// =========================================================================

fn run_parse_docx(args: ParseArgs) -> Result<()> {
    use blazegraph_io_core::preprocessors::docx::parse_docx;
    use blazegraph_io_core::preprocessors::md::ParseOptions;

    println!("📄 Parsing DOCX: {}", args.input);

    // The lib's `parse_docx` takes raw zip bytes (no pre-read in
    // `detect_input_format`, mirroring the PDF arm).
    let bytes =
        std::fs::read(&args.input).map_err(|e| anyhow!("failed to read {}: {e}", args.input))?;

    // `ParseOptions` is shared with the markdown channel; `accept_drift`
    // is meaningless for DOCX (no embedded `graph_sha256` to verify),
    // but we thread it through for API symmetry.
    let opts = ParseOptions {
        accept_drift: args.accept_drift,
    };
    let result = parse_docx(&bytes, opts).map_err(|e| anyhow!("\n❌ DOCX parse failed: {e}\n"))?;

    let graph = result.graph;
    let mut provenance = result.provenance;

    // The lib leaves `source_filename` empty — same convention as the
    // markdown channel; the CLI owns the filename. Overwrite the
    // provenance's `source_filename` with the input basename. Block A
    // bonus: provenance is envelope-only now, so this CLI-side override
    // can no longer perturb `graph_sha256`.
    provenance.source_filename = Path::new(&args.input)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&args.input)
        .to_string();

    emit_parsed_graph(&args, graph, provenance)
}

/// Shared emit/output tail for the markdown and DOCX channels: log the
/// node count, resolve the output path by `-f`, and write via
/// `save_graph` (graph / sequential / flat / markdown / bgraph-md).
///
/// Factored out of `run_parse_markdown` so the DOCX path emits through
/// the identical logic — every input parses to the channel-agnostic
/// canonical graph first, and the emit format is chosen by `-f` alone
/// (the B6 routing rule). The PDF channel keeps its own success-branch
/// tail because it carries extra concerns (JVM fast-exit, stage dump).
fn emit_parsed_graph(
    args: &ParseArgs,
    graph: DocumentGraph,
    provenance: ParseProvenance,
) -> Result<()> {
    println!("📊 Graph: {} nodes", graph.nodes.len());

    // CR-59 (v2.1.0+): style emission is opt-in. Pipeline keeps
    // `node.style_info` populated for library consumers; the bgraph.md
    // serializer is gated via `EmitOptions::include_style_info`.
    let output_path = resolve_output_path(args);
    save_graph(
        &graph,
        &provenance,
        &output_path,
        &args.output_format,
        args.include_style_info,
    )?;
    Ok(())
}

// =========================================================================
// `strip` subcommand
// =========================================================================

fn run_strip(args: StripArgs) -> Result<()> {
    use blazegraph_io_core::preprocessors::md::StripMode;

    let content = std::fs::read_to_string(&args.input)
        .map_err(|e| anyhow!("failed to read {}: {e}", args.input))?;

    // Run order (CR-55): if `--node-types` non-empty, apply the
    // structural-rule deletion pass first, then the `--mode` pass on
    // the result. The structural-rule pass walks body-above + fence
    // pair as a paired range deletion; the mode passes work on
    // remaining fences without needing the deleted ranges.
    let after_filter = if args.node_types.is_empty() {
        content
    } else {
        run_strip_step(&content, StripMode::NodeTypes(args.node_types.clone()))
    };
    let stripped = run_strip_step(&after_filter, args.mode.into());

    match &args.output {
        Some(path) => {
            std::fs::write(path, &stripped)
                .map_err(|e| anyhow!("failed to write {}: {e}", path))?;
            let mode_label = match args.mode {
                CliStripMode::BodyWithFrontmatter => "body-with-frontmatter",
                CliStripMode::BodyOnly => "body-only",
            };
            let filter_label = if args.node_types.is_empty() {
                String::new()
            } else {
                format!(" + --node-types={}", args.node_types.join(","))
            };
            println!(
                "💾 Stripped ({mode_label}{filter_label}, {} bytes) saved to: {path}",
                stripped.len()
            );
        }
        None => {
            // stdout, no decoration
            print!("{stripped}");
        }
    }
    Ok(())
}

/// Run a single strip pass, exiting the process on error. Used by
/// `run_strip` to compose the optional `--node-types` filter pass
/// with the `--mode` pass.
fn run_strip_step(content: &str, mode: blazegraph_io_core::preprocessors::md::StripMode) -> String {
    use blazegraph_io_core::preprocessors::md::{strip, ParseError};
    match strip(content, mode) {
        Ok(s) => s,
        Err(ParseError::MalformedFence(msg)) => {
            eprintln!("❌ strip failed: malformed bgraph fence — {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("❌ strip failed: {e}");
            std::process::exit(1);
        }
    }
}

// =========================================================================
// Helpers
// =========================================================================

/// Detected input shape. The `Markdown` variant carries the loaded
/// file contents so the caller doesn't re-read; the lib's
/// `parse_markdown` dispatcher decides between bgraph.md and generic
/// markdown internally.
enum InputFormat {
    /// PDF — dispatch to the existing PDF channel. Bytes are loaded
    /// lazily by the PDF processor itself; we do not pre-read here.
    Pdf,
    /// Markdown input (either bgraph.md round-trip artifact or
    /// generic markdown). The lib's `parse_markdown` dispatcher
    /// content-sniffs and routes; the CLI just passes the bytes.
    Markdown { content: String },
    /// DOCX input (OOXML WordprocessingML). Like the PDF variant, the
    /// bytes are read in the dispatched `run_parse_docx` (the lib's
    /// `parse_docx` takes the raw zip bytes), so this variant carries
    /// no payload — extension detection alone selects it.
    Docx,
    /// Unknown extension and content does not look like markdown.
    Unknown,
}

/// Resolve input format by extension first, content-sniff for `.md` /
/// unknown extensions.
///
/// - `.pdf` → `InputFormat::Pdf` (no file read; PDF pipeline reads bytes itself).
/// - `.bgraph.md` → read content, return `BgraphMd { content }` (the sniff
///   would always pass on a valid round-trip artifact, but we still read
///   the file so the caller can pass `&content` straight through).
/// - `.md`, `.markdown` → read content; sniff with
///   `is_bgraph_md` to distinguish round-trip artifact from generic
///   prose. Plain markdown is `Unknown` (we surface a clear error rather
///   than silently routing through the PDF channel).
/// - Unknown extension → try reading as UTF-8 and content-sniff. If it
///   sniffs as bgraph.md, treat it as such; otherwise `Unknown`.
fn detect_input_format(path: &Path) -> Result<InputFormat> {
    use blazegraph_io_core::preprocessors::md::is_bgraph_md;

    let ext_lower = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());

    match ext_lower.as_deref() {
        Some("pdf") => Ok(InputFormat::Pdf),
        // Extension is decisive for DOCX — no content sniff. The bytes
        // are read in `run_parse_docx` (the lib's `parse_docx` takes
        // raw zip bytes), mirroring the PDF arm's "read in the channel"
        // shape rather than the markdown arm's pre-read.
        Some("docx") => Ok(InputFormat::Docx),
        Some("md") | Some("markdown") => {
            let content = std::fs::read_to_string(path)
                .map_err(|e| anyhow!("failed to read {}: {e}", path.display()))?;
            // Both bgraph.md and generic markdown flow through the
            // lib's `parse_markdown` dispatcher — no CLI-level
            // distinction needed.
            Ok(InputFormat::Markdown { content })
        }
        _ => {
            // Unknown extension — content sniff. If the file isn't valid
            // UTF-8 (likely binary), bail to Unknown rather than reading
            // an arbitrary-size binary into memory just to detect it.
            let bytes = std::fs::read(path)
                .map_err(|e| anyhow!("failed to read {}: {e}", path.display()))?;
            let Ok(content) = String::from_utf8(bytes) else {
                return Ok(InputFormat::Unknown);
            };
            if is_bgraph_md(&content) {
                Ok(InputFormat::Markdown { content })
            } else {
                Ok(InputFormat::Unknown)
            }
        }
    }
}

/// Resolve the output path for `parse`. Explicit `-o` wins; otherwise
/// derive from input stem + a format-aware suffix.
///
/// Suffix table:
/// - `markdown` → `.bgraph.md`
/// - everything else → `_blazegraph.json`
///
/// The `config` suffix (`_{config_stem}`) is preserved from pre-B5
/// behavior on the JSON formats; it is intentionally NOT applied to
/// markdown output (the round-trip artifact's identity is the
/// `config_hash` embedded in the doc-level block, not a filename
/// suffix).
fn resolve_output_path(args: &ParseArgs) -> String {
    if let Some(output) = &args.output {
        return output.clone();
    }
    let input_name = Path::new(&args.input)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    // For .bgraph.md inputs, `file_stem()` strips the trailing `.md`
    // but leaves the `.bgraph` infix. That's intentional — the
    // intermediate `.bgraph` carries provenance information for
    // human readers (e.g., `paper.bgraph.json` is clearly derived
    // from `paper.bgraph.md`).
    //
    // Output-suffix table (B6):
    // - `-f markdown` (generic) → `.md`
    // - `-f bgraph-md` → `.bgraph.md`
    // - everything else (graph/sequential/flat) → `_blazegraph.json`
    match args.output_format.as_str() {
        "markdown" => return format!("{input_name}.md"),
        "bgraph-md" => return format!("{input_name}.bgraph.md"),
        _ => {}
    }
    let config_suffix = args
        .config
        .as_ref()
        .and_then(|p| Path::new(p).file_stem())
        .and_then(|s| s.to_str())
        .map(|s| format!("_{s}"))
        .unwrap_or_default();
    format!("{input_name}{config_suffix}_blazegraph.json")
}

/// Resolve cache directory: CLI flag > env var > default (~/.local/share/blazegraph/cache/)
fn resolve_cache_dir(args: &ParseArgs) -> Result<String> {
    if let Some(ref dir) = args.cache_dir {
        return Ok(dir.clone());
    }
    if let Ok(dir) = std::env::var("BLAZEGRAPH_CACHE_DIR") {
        if !dir.is_empty() {
            return Ok(dir);
        }
    }
    // Default: ~/.local/share/blazegraph/cache/
    #[cfg(feature = "jni-backend")]
    {
        let data_dir = JreManager::get_data_dir()?;
        Ok(data_dir.join("cache").to_string_lossy().to_string())
    }
    #[cfg(not(feature = "jni-backend"))]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Ok(format!("{}/.local/share/blazegraph/cache", home))
    }
}

/// Create DocumentProcessor with JNI backend (cross-platform, auto-downloads JRE)
#[cfg(feature = "jni-backend")]
fn create_processor(args: &ParseArgs, cache_dir: &str) -> Result<DocumentProcessor> {
    // Get JRE path - either from args, JAVA_HOME, or auto-download
    let jre_path = if let Some(path) = &args.jre_path {
        println!("🔧 Using specified JRE: {}", path);
        std::path::PathBuf::from(path)
    } else if let Ok(java_home) = std::env::var("JAVA_HOME") {
        if !java_home.is_empty() {
            println!("🔧 Using JAVA_HOME: {}", java_home);
            std::path::PathBuf::from(java_home)
        } else {
            let jre_manager = JreManager::new()?;
            jre_manager.ensure_jre()?
        }
    } else {
        let jre_manager = JreManager::new()?;
        jre_manager.ensure_jre()?
    };

    // Get JAR path - either from args or find bundled JAR
    let jar_path = if let Some(path) = &args.jar_path {
        println!("🔧 Using specified JAR: {}", path);
        std::path::PathBuf::from(path)
    } else {
        let path = JreManager::find_jar_path()?;
        println!("🔧 Using JAR: {}", path.display());
        path
    };

    println!("🚀 Using JNI backend");
    DocumentProcessor::new_cli_jni_with_cache(&jre_path, &jar_path, cache_dir)
}

/// Fallback when no backend is compiled in
#[cfg(not(feature = "jni-backend"))]
fn create_processor(_args: &ParseArgs, _cache_dir: &str) -> Result<DocumentProcessor> {
    Err(anyhow::anyhow!(
        "No PDF backend compiled in!\n\
         Compile with: --features jni-backend"
    ))
}

fn show_help() {
    println!("\n📋 Subcommands:");
    println!("  parse    Parse a PDF or bgraph.md into a graph (or back to markdown)");
    println!("  strip    Remove bgraph fences from a bgraph.md file");

    println!("\n📋 `parse` options:");
    println!("  --config <path>         Load custom config file (PDF only)");
    println!("  --input <path>          Input file (PDF, .docx, .bgraph.md, or .md)");
    println!("  --output <path>         Output file path (auto-generated if not specified)");
    println!("  --output-format <fmt>   Output format: graph, sequential, flat, or markdown");
    println!("  --accept-drift          Accept hash-drifted bgraph.md input (returns derivative)");
    println!("  --minimal-parse         Enable minimal parse mode (PDF only)");
    println!("  --jre-path <path>       Path to JRE directory (default: auto-download)");
    println!("  --jar-path <path>       Path to Tika JAR file (default: bundled)");

    println!("\n🗄️  Cache Control (PDF only):");
    println!("  --cache-dir <path>      Override cache directory");
    println!("  --fresh-from <point>    Reprocess from cache point: c0, c1, c2, c3");
    println!("  --clear-cache <point>   Clear cache (cascading): c0, c1, c2, c3, all");
    println!("  --skip-cache            Alias for --fresh-from c0");

    println!("\n📄 Output Formats (parse):");
    println!("  graph       - Full graph structure with nodes and relationships (default)");
    println!("  sequential  - Ordered segments with level info (good for RAG + hierarchy)");
    println!("  flat        - Simple array of text chunks (minimal format)");
    println!("  markdown    - Plain markdown (generic) — B6, schema 0.7.0+");
    println!("  bgraph-md   - bgraph.md round-trip artifact (B2; was -f markdown in B5)");

    println!("\n📥 Input Formats (parse, auto-detected):");
    println!("  .pdf                    PDF channel (full pipeline)");
    println!("  .docx                   Word/OOXML channel (S10 Track C)");
    println!("  .bgraph.md              bgraph.md round-trip artifact");
    println!("  .md / .markdown         Generic markdown (B6)");

    println!("\n🪓 `strip` modes:");
    println!(
        "  --mode body-with-frontmatter  (default) Strip every fence; lift doc-level metadata to YAML frontmatter"
    );
    println!(
        "  --mode body-only        Remove all bgraph fences (Unstructured-equivalent body output)"
    );
    println!(
        "  --node-types <list>     Comma-sep types to strip entirely via structural rule (e.g. header,footer,margin)"
    );

    println!("\n📝 Usage Examples:");
    println!("  blazegraph parse -i document.pdf");
    println!("  blazegraph parse -i document.docx");
    println!("  blazegraph parse -i document.docx -f markdown -o document.md");
    println!("  blazegraph parse -i document.pdf -f bgraph-md -o document.bgraph.md");
    println!("  blazegraph parse -i document.md -f markdown -o roundtrip.md");
    println!("  blazegraph parse -i document.md -f graph -o document.json");
    println!("  blazegraph parse -i document.bgraph.md -o document.json");
    println!("  blazegraph parse -i document.bgraph.md --accept-drift -o derived.json");
    println!(
        "  blazegraph strip -i document.bgraph.md -o document.md   # default: body+frontmatter"
    );
    println!("  blazegraph strip -i document.bgraph.md --mode body-only -o document_body.md");
    println!(
        "  blazegraph strip -i document.bgraph.md --node-types header,footer,margin -o clean.md"
    );

    #[cfg(feature = "jni-backend")]
    {
        println!("\n🔧 JNI Backend:");
        println!(
            "  First run will auto-download Java Runtime (~60MB) to ~/.local/share/blazegraph/jre"
        );
        println!("  Or specify your own JRE: --jre-path /path/to/jre");
    }
}

fn save_stages(stages: &PipelineStages, output_dir: &str) -> Result<()> {
    use std::fs;
    fs::create_dir_all(output_dir)?;

    // Stage 1a: Raw XHTML
    let xhtml_path = format!("{}/stage1a_xhtml.html", output_dir);
    fs::write(&xhtml_path, &stages.xhtml)?;
    println!("  💾 {}", xhtml_path);

    // Stage 1b: TextElements
    let te_path = format!("{}/stage1b_text_elements.json", output_dir);
    let te_json = serde_json::to_string_pretty(&stages.text_elements)?;
    fs::write(&te_path, &te_json)?;
    println!("  💾 {} ({} elements)", te_path, stages.text_elements.len());

    // Stage 2: ParsedElements
    let pe_path = format!("{}/stage2_parsed_elements.json", output_dir);
    let pe_json = serde_json::to_string_pretty(&stages.parsed_elements)?;
    fs::write(&pe_path, &pe_json)?;
    println!(
        "  💾 {} ({} elements)",
        pe_path,
        stages.parsed_elements.len()
    );

    // Stage 3: Final graph
    let graph_path = format!("{}/stage3_graph.json", output_dir);
    // Stage-dump graphs come from the legacy provenance-free build path.
    stages.graph.save_with_format(&graph_path, "graph", None)?;
    println!("  💾 {} ({} nodes)", graph_path, stages.graph.nodes.len());

    // Summary file
    let summary = serde_json::json!({
        "captured_at": chrono::Utc::now().to_rfc3339(),
        "stage_counts": {
            "xhtml_bytes": stages.xhtml.len(),
            "text_elements": stages.text_elements.len(),
            "parsed_elements": stages.parsed_elements.len(),
            "graph_nodes": stages.graph.nodes.len(),
        }
    });
    let summary_path = format!("{}/summary.json", output_dir);
    fs::write(&summary_path, serde_json::to_string_pretty(&summary)?)?;
    println!("  💾 {}", summary_path);

    Ok(())
}

fn save_graph(
    graph: &DocumentGraph,
    provenance: &ParseProvenance,
    output_path: &str,
    format: &str,
    include_style_info: bool,
) -> Result<()> {
    match format {
        // B6: `-f markdown` is the generic-markdown emitter.
        // `-f bgraph-md` (new) is the bgraph.md round-trip artifact
        // emitter (which was `-f markdown` in B5).
        //
        // Routing is by `-f` alone — never by the input source. A
        // PDF parsed through the full pipeline can emit to either
        // markdown variant; a bgraph.md input parsed back can emit
        // to either; etc.
        "markdown" => {
            // Pre-emit validation: the generic emitter panics on
            // Header/Footer/Margin (PDF-only variants that don't
            // exist in generic markdown source). Catch them here
            // with a clean error.
            if let Err(msg) = check_generic_md_compatible(graph) {
                eprintln!(
                    "\n❌ Cannot emit generic markdown: {msg}\n\
                     \n\
                     Tip: use `-f bgraph-md` instead to produce the bgraph.md\n\
                     round-trip artifact, which carries Header/Footer/Margin\n\
                     fences.\n"
                );
                std::process::exit(2);
            }
            let md =
                blazegraph_io_core::graphs::serialization::markdown_generic::emit_markdown(graph);
            std::fs::write(output_path, md)?;
            println!("💾 Markdown saved to: {}", output_path);
        }
        "bgraph-md" => {
            // CR-59 (v2.1.0+): the bgraph.md emitter takes an explicit
            // options struct; the CLI threads `--include-style-info`
            // through. Other output formats don't carry style on the
            // wire (graph.json carries it via `DocumentNode.style_info`
            // directly; generic markdown has no per-element JSON).
            let opts = blazegraph_io_core::graphs::serialization::markdown::EmitOptions {
                include_style_info,
            };
            let md =
                blazegraph_io_core::graphs::serialization::markdown::emit_markdown_with_options(
                    graph, provenance, opts,
                );
            std::fs::write(output_path, md)?;
            println!("💾 bgraph.md saved to: {}", output_path);
        }
        "sequential" => {
            graph.save_with_format(output_path, "sequential", Some(provenance))?;
            println!("💾 Sequential format saved to: {}", output_path);
        }
        "flat" => {
            graph.save_with_format(output_path, "flat", Some(provenance))?;
            println!("💾 Flat format saved to: {}", output_path);
        }
        "graph" => {
            graph.save_with_format(output_path, "graph", Some(provenance))?;
            println!("💾 Graph saved to: {}", output_path);
        }
        other => {
            println!("⚠️  Unknown output format '{other}', using default graph format");
            graph.save_with_format(output_path, "graph", Some(provenance))?;
            println!("💾 Graph saved to: {}", output_path);
        }
    }
    Ok(())
}

/// Pre-emit validation for the generic-markdown emitter.
///
/// Returns `Err` if the graph contains any PDF-only variants
/// (`Header`, `Footer`, `Margin`) that have no representation in
/// plain markdown source. The lib's `markdown_generic::emit_markdown`
/// panics on these variants as a defense-in-depth safety net; this
/// CLI-layer check catches them earlier with a clean message.
///
/// Visible at module scope so tests can exercise it directly.
fn check_generic_md_compatible(graph: &DocumentGraph) -> std::result::Result<(), String> {
    let mut incompatible: Vec<&str> = graph
        .nodes
        .values()
        .filter(|n| matches!(n.node_type.as_str(), "Header" | "Footer" | "Margin"))
        .map(|n| n.node_type.as_str())
        .collect();
    if incompatible.is_empty() {
        return Ok(());
    }
    incompatible.sort();
    incompatible.dedup();
    Err(format!(
        "graph contains PDF-only variant(s) [{}] that have no representation \
         in generic markdown source",
        incompatible.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use blazegraph_io_core::types::*;
    use std::collections::HashMap;
    use uuid::Uuid;

    /// Build a tiny graph with a Document root + supplied body nodes.
    /// `nodes_in` is `(node_type, text_order)`.
    fn graph_with_types(node_types: &[&str]) -> DocumentGraph {
        let root_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"cli-test-root");
        let mut nodes = HashMap::new();
        let mut child_ids = Vec::new();
        for (i, nt) in node_types.iter().enumerate() {
            let id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, format!("cli:{i}").as_bytes());
            child_ids.push(id);
            nodes.insert(
                id,
                DocumentNode {
                    id,
                    node_type: nt.to_string(),
                    location: NodeLocation {
                        semantic: SemanticLocation {
                            path: format!("{}", i + 1),
                            depth: 1,
                            breadcrumbs: Vec::new(),
                        },
                        physical: None,
                    },
                    text_order: Some(i as u32),
                    content: NodeContent {
                        text: nt.to_string(),
                    },
                    style_info: None,
                    token_count: 1,
                    parent: Some(root_id),
                    children: Vec::new(),
                    internal_refs: Vec::new(),
                    external_refs: Vec::new(),
                    confidence: 0,
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
                    text: "Document".to_string(),
                },
                style_info: None,
                token_count: 0,
                parent: None,
                children: child_ids,
                internal_refs: Vec::new(),
                external_refs: Vec::new(),
                confidence: 0,
            },
        );
        DocumentGraph {
            nodes,
            document_info: DocumentInfo {
                root_id,
                kind: blazegraph_io_core::types::default_kind(),
                document_metadata: DocumentMetadata::default(),
                outline_data: None,
                topology: None,
            },
            structural_profile: StructuralProfile::default(),
        }
    }

    #[test]
    fn check_generic_md_compatible_accepts_markdown_variants() {
        // Every variant that has a plain-markdown source-form (which
        // covers Document/Section/Paragraph plus the four B6
        // additions) passes the check.
        let graph = graph_with_types(&[
            "Section",
            "Paragraph",
            "CodeBlock",
            "List",
            "Blockquote",
            "Table",
        ]);
        assert!(check_generic_md_compatible(&graph).is_ok());
    }

    #[test]
    fn check_generic_md_compatible_rejects_header_footer_margin() {
        for variant in ["Header", "Footer", "Margin"] {
            let graph = graph_with_types(&[variant, "Paragraph"]);
            let err = check_generic_md_compatible(&graph)
                .expect_err(&format!("variant {variant} should reject"));
            assert!(
                err.contains(variant),
                "error message should name the variant `{variant}`; got: {err}"
            );
        }
    }

    #[test]
    fn check_generic_md_compatible_rejects_multiple_pdf_only_variants() {
        let graph = graph_with_types(&["Header", "Footer", "Margin", "Paragraph"]);
        let err = check_generic_md_compatible(&graph)
            .expect_err("multi-variant PDF-only graph should reject");
        for variant in ["Header", "Footer", "Margin"] {
            assert!(
                err.contains(variant),
                "error should list all incompatible variants; missing `{variant}` in: {err}"
            );
        }
    }
}

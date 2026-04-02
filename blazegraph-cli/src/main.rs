use anyhow::Result;
use clap::Parser;
use std::path::Path;

// Import from blazegraph-io-core
use blazegraph_io_core::{
    CacheDefaults, CachePoint, DocumentGraph, DocumentProcessor, FreshFrom, ParsingConfig,
    PipelineStages,
};

/// Default config embedded at compile time — guarantees every install has working defaults.
/// Without this, `cargo install` users get raw parse output (3000+ nodes, 0 sections).
const DEFAULT_CONFIG_YAML: &str = include_str!("../configs/processing/config.yaml");

// Import CLI utilities
#[cfg(feature = "jni-backend")]
use blazegraph_io::JreManager;

#[derive(Parser)]
#[command(name = "blazegraph")]
#[command(about = "A semantic document graph parser with configurable rules")]
struct Args {
    /// Path to the PDF file to process
    #[arg(short, long, default_value = "../sample_pdfs/sample3.pdf")]
    input: String,

    /// Path to custom config file (YAML format)
    #[arg(short, long)]
    config: Option<String>,

    /// Output format: graph, sequential, or flat
    #[arg(short = 'f', long, default_value = "graph")]
    output_format: String,

    /// Show available config options and exit
    #[arg(long)]
    show_configs: bool,

    /// Output file path (if not specified, auto-generated based on input)
    #[arg(short, long)]
    output: Option<String>,

    /// Enable minimal parse mode (bypass all rule processing)
    #[arg(long)]
    minimal_parse: bool,

    /// Path to JRE directory (for JNI backend)
    /// If not specified, JRE will be auto-downloaded on first use
    #[arg(long)]
    jre_path: Option<String>,

    /// Path to Tika JAR file (for JNI backend)
    /// If not specified, uses bundled JAR
    #[arg(long)]
    jar_path: Option<String>,

    /// Enable detailed profiling of all pipeline steps
    #[arg(long)]
    profile: bool,

    /// Include style_info on each node (font_class, font_size, font_family, bold, italic, color).
    /// Stripped by default to reduce output size (~20%). Useful for authoring parsing configs.
    #[arg(long)]
    include_style_info: bool,

    /// Dump all intermediate pipeline stage outputs to a directory
    /// Captures: XHTML, TextElements, ParsedElements, and final Graph as separate files
    #[arg(long)]
    dump_stages: bool,

    /// Directory for stage dump output (default: {cache_dir}/debug)
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

    /// Alias for --fresh-from c0 (reprocess everything from scratch)
    #[arg(long)]
    skip_cache: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

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
        println!("⚠️  Input PDF not found at: {}", args.input);
        println!("   Please check the file path.");
        return Ok(());
    }

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
        let stages_dir = args.stages_dir.unwrap_or_else(|| format!("{}/debug", cache_dir));
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
        Ok(mut graph) => {
            println!("✅ Successfully processed document");
            println!("📊 Graph: {} nodes", graph.nodes.len());

            // Strip style_info from output unless explicitly requested
            if !args.include_style_info {
                for node in graph.nodes.values_mut() {
                    node.style_info = None;
                }
            }

            // Generate output path
            let output_path = if let Some(output) = &args.output {
                output.clone()
            } else {
                let input_name = Path::new(&args.input)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("output");
                let config_suffix = args
                    .config
                    .as_ref()
                    .and_then(|p| Path::new(p).file_stem())
                    .and_then(|s| s.to_str())
                    .map(|s| format!("_{s}"))
                    .unwrap_or_default();
                format!("{input_name}{config_suffix}_blazegraph.json")
            };

            // Save the graph
            save_graph(&graph, &output_path, &args.output_format)?;

            // Fast exit - skip JVM shutdown sequence
            #[cfg(feature = "jni-backend")]
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("❌ Processing failed: {e}");
            std::process::exit(1);
        }
    }
}

/// Resolve cache directory: CLI flag > env var > default (~/.local/share/blazegraph/cache/)
fn resolve_cache_dir(args: &Args) -> Result<String> {
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
fn create_processor(args: &Args, cache_dir: &str) -> Result<DocumentProcessor> {
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
fn create_processor(_args: &Args, _cache_dir: &str) -> Result<DocumentProcessor> {
    Err(anyhow::anyhow!(
        "No PDF backend compiled in!\n\
         Compile with: --features jni-backend"
    ))
}

fn show_help() {
    println!("\n📋 Available Configuration Options:");
    println!("  --config <path>         Load custom config file");
    println!("  --input <path>          PDF file to process");
    println!("  --output <path>         Output file path (auto-generated if not specified)");
    println!("  --output-format <fmt>   Output format: graph, sequential, or flat");
    println!("  --minimal-parse         Enable minimal parse mode (bypass all rule processing)");
    println!("  --jre-path <path>       Path to JRE directory (default: auto-download)");
    println!("  --jar-path <path>       Path to Tika JAR file (default: bundled)");

    println!("\n🗄️  Cache Control:");
    println!("  --cache-dir <path>      Override cache directory (default: ~/.local/share/blazegraph/cache/)");
    println!("  --fresh-from <point>    Reprocess from cache point: c0, c1, c2, c3");
    println!("  --clear-cache <point>   Clear cache (cascading): c0, c1, c2, c3, all");
    println!("  --skip-cache            Alias for --fresh-from c0");

    println!("\n📄 Output Formats:");
    println!("  graph       - Full graph structure with nodes and relationships (default)");
    println!("  sequential  - Ordered segments with level info (good for RAG + hierarchy)");
    println!("  flat        - Simple array of text chunks (minimal format)");

    println!("\n📁 Example config files in ./configs/:");
    println!("  generic-conservative.yaml  - Fewer, higher-confidence sections");
    println!("  generic-balanced.yaml      - Balanced section detection");
    println!("  generic-aggressive.yaml    - More sections, deeper hierarchy");

    println!("\n📝 Usage Examples:");
    println!("  cargo run -- -i document.pdf");
    println!("  cargo run -- -i document.pdf -o /path/to/output.json");
    println!("  cargo run -- -i document.pdf -c config.yaml -f sequential");
    println!("  cargo run -- --fresh-from c2    # reparse from cached XHTML");
    println!("  cargo run -- --clear-cache c1   # clear XHTML + downstream caches");

    #[cfg(feature = "jni-backend")]
    {
        println!("\n🔧 JNI Backend:");
        println!("  First run will auto-download Java Runtime (~60MB) to ~/.local/share/blazegraph/jre");
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
    println!("  💾 {} ({} elements)", pe_path, stages.parsed_elements.len());

    // Stage 3: Final graph
    let graph_path = format!("{}/stage3_graph.json", output_dir);
    stages.graph.save_with_format(&graph_path, "graph")?;
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

fn save_graph(graph: &DocumentGraph, output_path: &str, format: &str) -> Result<()> {
    graph.save_with_format(output_path, format)?;

    match format {
        "sequential" => println!("💾 Sequential format saved to: {}", output_path),
        "flat" => println!("💾 Flat format saved to: {}", output_path),
        "graph" => println!("💾 Graph saved to: {}", output_path),
        _ => {
            println!(
                "⚠️  Unknown output format '{}', using default graph format",
                format
            );
            println!("💾 Graph saved to: {}", output_path);
        }
    }

    Ok(())
}

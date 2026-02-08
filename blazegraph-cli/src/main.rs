use anyhow::Result;
use clap::Parser;
use std::path::Path;

// Import from blazegraph-core
use blazegraph_core::{DocumentProcessor, DocumentGraph, ParsingConfig, PipelineStages};

// Import CLI utilities
#[cfg(feature = "jni-backend")]
use blazegraph_cli::JreManager;

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

    /// Include raw Tika XML/HTML output in graph metadata for debugging
    #[arg(long)]
    include_raw_tika: bool,

    /// Output directory for raw tika files (when using --include-raw-tika)  
    #[arg(long)]
    output_dir: Option<String>,

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

    /// Skip cache and force fresh processing (useful for development/testing)
    #[arg(long)]
    skip_cache: bool,

    /// Include style_info on each node (font_class, font_size, font_family, bold, italic, color).
    /// Stripped by default to reduce output size (~20%). Useful for authoring parsing configs.
    #[arg(long)]
    include_style_info: bool,

    /// Dump all intermediate pipeline stage outputs to a directory
    /// Captures: XHTML, TextElements, ParsedElements, and final Graph as separate files
    #[arg(long)]
    dump_stages: bool,

    /// Directory for stage dump output (default: test_outputs/stages)
    #[arg(long, default_value = "test_outputs/stages")]
    stages_dir: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("🦀 Blazegraph Document Parser");

    if args.show_configs {
        show_help();
        return Ok(());
    }

    // Check if input file exists
    if !Path::new(&args.input).exists() {
        println!("⚠️  Input PDF not found at: {}", args.input);
        println!("   Please check the file path.");
        return Ok(());
    }

    // Create processor based on available backend
    let mut processor = create_processor(&args)?;

    // Load config using new functional pattern
    let mut config = ParsingConfig::load_with_fallback(args.config.as_deref());
    
    if let Some(config_path) = &args.config {
        println!("📋 Loaded config from: {}", config_path);
    } else {
        println!("📋 Using default config");
    }

    // Apply CLI overrides to config
    if args.include_raw_tika {
        config.include_raw_tika = true;
    }
    if args.minimal_parse {
        config.minimal_parse = true;
    }

    println!("📄 Processing: {}", args.input);

    // Stage dump mode: capture and save all intermediates
    if args.dump_stages {
        println!("\n🔬 Pipeline stage dump mode");
        match processor.process_document_capture_stages(&args.input, &config) {
            Ok(stages) => {
                save_stages(&stages, &args.stages_dir)?;
                println!("\n✅ All stages dumped to: {}", args.stages_dir);
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

    // Process the document with config flow (and profiling if enabled)
    match processor.process_document_with_config_and_profiling(&args.input, &config, args.profile, args.skip_cache)
    {
        Ok(mut graph) => {
            println!("✅ Successfully processed document");
            println!("📊 Graph metrics:");
            println!("   - Nodes: {}", graph.nodes.len());

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
            
            // Fast exit - skip JVM shutdown sequence (finalizers, GC)
            // The OS reclaims all memory instantly anyway
            #[cfg(feature = "jni-backend")]
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("❌ Processing failed: {e}");
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Create DocumentProcessor with JNI backend (cross-platform, auto-downloads JRE)
#[cfg(feature = "jni-backend")]
fn create_processor(args: &Args) -> Result<DocumentProcessor> {
    // Get JRE path - either from args, JAVA_HOME, or auto-download
    let jre_path = if let Some(path) = &args.jre_path {
        // User specified JRE path
        println!("🔧 Using specified JRE: {}", path);
        std::path::PathBuf::from(path)
    } else if let Ok(java_home) = std::env::var("JAVA_HOME") {
        // Use JAVA_HOME if set and non-empty
        if !java_home.is_empty() {
            println!("🔧 Using JAVA_HOME: {}", java_home);
            std::path::PathBuf::from(java_home)
        } else {
            // JAVA_HOME is empty, auto-download
            let jre_manager = JreManager::new()?;
            jre_manager.ensure_jre()?
        }
    } else {
        // Auto-download JRE if not available
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
    DocumentProcessor::new_cli_jni(&jre_path, &jar_path)
}

/// Fallback when no backend is compiled in
#[cfg(not(feature = "jni-backend"))]
fn create_processor(_args: &Args) -> Result<DocumentProcessor> {
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
    println!("  --include-raw-tika      Include raw Tika XML/HTML output in graph metadata for debugging");
    println!("  --minimal-parse         Enable minimal parse mode (bypass all rule processing)");
    println!("  --jre-path <path>       Path to JRE directory (default: auto-download)");
    println!("  --jar-path <path>       Path to Tika JAR file (default: bundled)");
    
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

    // Summary file: quick reference for validation scripts
    let summary = serde_json::json!({
        "input_pdf": "claude_shannon_paper.pdf",
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
    // Use the existing save_with_format method from DocumentGraph
    graph.save_with_format(output_path, format)?;
    
    match format {
        "sequential" => println!("💾 Sequential format results saved to: {}", output_path),
        "flat" => println!("💾 Flat format results saved to: {}", output_path),
        "graph" => println!("💾 Graph format results saved to: {}", output_path),
        _ => {
            println!("⚠️  Unknown output format '{}', using default graph format", format);
            println!("💾 Graph format results saved to: {}", output_path);
        }
    }
    
    Ok(())
}

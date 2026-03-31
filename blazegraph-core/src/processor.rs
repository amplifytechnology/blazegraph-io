use crate::cache::GraphCacheKey;
// NOTE: GraphCacheValue unused while L2 cache is disabled (CR-07)
// use crate::cache::GraphCacheValue;
use crate::classifier::DocumentClassifier;
use crate::config::ParsingConfig;
use crate::graphs::builder::GraphBuilder;
use crate::preprocessors::{Preprocessor, TikaPreprocessor};
use crate::rules::{engine::DebugConfig, RuleEngine};
use crate::storage::{calculate_config_hash, calculate_pdf_hash, DocumentStorage, FileStorage};
use crate::types::*;
use anyhow::Result;
use std::path::Path;
use std::time::{Duration, Instant};

/// Captured intermediate outputs from each pipeline stage
/// Used for testing and diagnostics — lets you inspect/compare each boundary
#[derive(Debug, Clone, serde::Serialize)]
pub struct PipelineStages {
    pub xhtml: String,
    pub text_elements: Vec<PdfTextElement>,
    pub parsed_elements: Vec<ParsedPdfElement>,
    pub graph: DocumentGraph,
}

/// Simple profiler that collects timings for pipeline steps
pub struct StepProfiler {
    enabled: bool,
    timings: Vec<(String, Duration)>,
}

impl StepProfiler {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            timings: Vec::new(),
        }
    }

    pub fn time_step<F, R>(&mut self, step_name: &str, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        if !self.enabled {
            return f();
        }

        let start = Instant::now();
        let result = f();
        let elapsed = start.elapsed();

        self.timings.push((step_name.to_string(), elapsed));
        println!("⏱️  {}: {:.0}ms", step_name, elapsed.as_millis());

        result
    }

    pub fn print_summary(&self) {
        if !self.enabled || self.timings.is_empty() {
            return;
        }

        println!("\n📊 Performance Summary:");
        let total: Duration = self.timings.iter().map(|(_, d)| *d).sum();

        for (step, duration) in &self.timings {
            let percentage = (duration.as_secs_f64() / total.as_secs_f64()) * 100.0;
            println!(
                "   {:.<35} {:.0}ms ({:.1}%)",
                step,
                duration.as_millis(),
                percentage
            );
        }
        println!("   {:.<35} {:.0}ms", "Total", total.as_millis());
    }
}

pub struct DocumentProcessor {
    preprocessor: Box<dyn Preprocessor>,
    storage: Box<dyn DocumentStorage + Send + Sync>,
    classifier: DocumentClassifier,
    rule_engine: RuleEngine,
    graph_builder: GraphBuilder,
}

impl DocumentProcessor {
    /// Create DocumentProcessor with full dependency injection
    pub fn new_with_dependencies(
        preprocessor: Box<dyn Preprocessor>,
        storage: Box<dyn DocumentStorage + Send + Sync>,
    ) -> Result<Self> {
        Ok(Self {
            preprocessor,
            storage,
            classifier: DocumentClassifier::new(),
            rule_engine: RuleEngine::new()?,
            graph_builder: GraphBuilder::new(),
        })
    }

    /// Convenience constructor for CLI usage with JNI backend (cross-platform)
    ///
    /// # Arguments
    /// * `jre_path` - Path to JRE directory
    /// * `jar_path` - Path to blazing-tika.jar
    #[cfg(feature = "jni-backend")]
    pub fn new_cli_jni(jre_path: &std::path::Path, jar_path: &std::path::Path) -> Result<Self> {
        let preprocessor = Box::new(TikaPreprocessor::new_with_jni(jre_path, jar_path)?);
        let storage = Box::new(FileStorage::new("cache")?);
        Self::new_with_dependencies(preprocessor, storage)
    }

    /// Convenience constructor for CLI with JNI backend and custom cache directory
    #[cfg(feature = "jni-backend")]
    pub fn new_cli_jni_with_cache(
        jre_path: &std::path::Path,
        jar_path: &std::path::Path,
        cache_dir: &str,
    ) -> Result<Self> {
        let preprocessor = Box::new(TikaPreprocessor::new_with_jni(jre_path, jar_path)?);
        let storage = Box::new(FileStorage::new(cache_dir)?);
        Self::new_with_dependencies(preprocessor, storage)
    }

    // Future: Convenience constructor for API usage (server Tika + database storage)
    // This will be implemented when server-based Tika preprocessor is available
    // pub fn new_api(server_url: &str, db_config: &DatabaseConfig) -> Result<Self> {
    //     let preprocessor = Box::new(TikaPreprocessor::new_with_server(server_url)?);
    //     let storage = Box::new(DatabaseStorage::new(db_config)?);
    //     Self::new_with_dependencies(preprocessor, storage)
    // }

    /// Process document with specific config and profiling (pure function approach)
    /// This is the main method implementing PDF + Config → Graph with Level 2 caching
    pub fn process_document_with_config_and_profiling(
        &mut self,
        input_path: &str,
        config: &ParsingConfig,
        enable_profiling: bool,
        skip_cache: bool,
    ) -> Result<DocumentGraph> {
        if enable_profiling {
            self.process_document_with_config_and_profiler(
                input_path,
                config,
                StepProfiler::new(true),
                skip_cache,
            )
        } else if skip_cache {
            // Skip cache without profiling - use no-op profiler
            self.process_document_with_config_and_profiler(
                input_path,
                config,
                StepProfiler::new(false),
                skip_cache,
            )
        } else {
            self.process_document_with_config(input_path, config)
        }
    }

    /// Process document with specific config (pure function approach)
    /// This is the main method implementing PDF + Config → Graph with Level 2 caching
    pub fn process_document_with_config(
        &mut self,
        input_path: &str,
        config: &ParsingConfig,
    ) -> Result<DocumentGraph> {
        let start_time = Instant::now();

        // Read PDF and calculate hash
        let pdf_bytes = std::fs::read(input_path)?;
        let pdf_hash = calculate_pdf_hash(&pdf_bytes);

        // NOTE: Level 2 cache key computed but unused while L2 cache is disabled (CR-07)
        let _config_hash = calculate_config_hash(config)?;
        let _cache_key = GraphCacheKey::new(pdf_hash.clone(), _config_hash);

        // NOTE: Level 2 graph cache disabled during config optimisation work (CR-07).
        // We want every config variant to run through the rule engine fresh.
        // Preprocessor cache (Level 1) remains active — Tika extraction is still cached.
        // TODO: Re-enable for production use.
        // if let Some(cached) = self.storage.get_graph_output(&cache_key)? {
        //     println!("🎯 Cache hit: Found graph for PDF + config combination");
        //     println!(
        //         "⏱️  Total processing time: {:.3}s (cached)",
        //         start_time.elapsed().as_secs_f64()
        //     );
        //     return Ok(cached.graph);
        // }

        println!("📄 Processing document with config: {}", input_path);

        // Process with config flow (pass pdf_hash for Level 1 preprocessor caching)
        let graph = self.process_with_config_flow(input_path, config, &pdf_hash)?;

        println!(
            "⏱️  Total processing time: {:.3}s",
            start_time.elapsed().as_secs_f64()
        );
        Ok(graph)
    }

    /// Process document with profiler for detailed timing
    fn process_document_with_config_and_profiler(
        &mut self,
        input_path: &str,
        config: &ParsingConfig,
        mut profiler: StepProfiler,
        _skip_cache: bool, // NOTE: unused while L2 cache is disabled (CR-07)
    ) -> Result<DocumentGraph> {
        let start_time = Instant::now();

        // Check cache first (timed)
        // NOTE: config_hash and cache_key computed but unused while L2 cache is disabled (CR-07)
        let (pdf_hash, _cache_key) = profiler.time_step("Cache Key Generation", || {
            let pdf_bytes = std::fs::read(input_path)?;
            let pdf_hash = calculate_pdf_hash(&pdf_bytes);
            let config_hash = calculate_config_hash(config)?;
            let cache_key = GraphCacheKey::new(pdf_hash.clone(), config_hash);
            Ok::<(String, GraphCacheKey), anyhow::Error>((pdf_hash, cache_key))
        })?;

        // NOTE: Level 2 graph cache disabled during config optimisation work (CR-07).
        // Preprocessor cache (Level 1) remains active.
        // TODO: Re-enable for production use.

        println!("📄 Processing document with config: {}", input_path);

        // Process with detailed profiling (pass pdf_hash for Level 1 preprocessor caching)
        let graph =
            self.process_with_config_flow_and_profiler(input_path, config, &mut profiler, &pdf_hash)?;

        profiler.print_summary();
        println!(
            "⏱️  Total processing time: {:.0}ms",
            start_time.elapsed().as_millis()
        );
        Ok(graph)
    }

    /// Internal processing with config flow through all pipeline stages
    fn process_with_config_flow(
        &mut self,
        input_path: &str,
        config: &ParsingConfig,
        pdf_hash: &str,
    ) -> Result<DocumentGraph> {
        let stage1_start = Instant::now();

        // Stage 1: Preprocessing (PDF → TextElements)
        // Check Level 1 preprocessor cache first (keyed by PDF hash alone, config-independent)
        let preprocessor_output =
            if let Some(cached) = self.storage.get_preprocessor_output(pdf_hash)? {
                println!(
                    "🎯 Preprocessor cache hit — skipping Tika extraction ({:.3}s)",
                    stage1_start.elapsed().as_secs_f64()
                );
                cached
            } else {
                let input_path = Path::new(input_path);
                let output = self.preprocessor.process_file(input_path)?;
                // Store in preprocessor cache for future config sweeps
                self.storage
                    .store_preprocessor_output(pdf_hash, &output)?;
                println!(
                    "⏱️  Preprocessing: {:.3}s (cached for future configs)",
                    stage1_start.elapsed().as_secs_f64()
                );
                output
            };

        let stage2_start = Instant::now();

        // Stage 2: Classification
        let classification = self.classifier.classify(&preprocessor_output)?;
        println!("📋 Document classified as: {:?}", classification);
        println!(
            "⏱️  Classification: {:.3}s",
            stage2_start.elapsed().as_secs_f64()
        );

        let stage3_start = Instant::now();

        // Compute document analysis once (used by rules and stored in DocumentInfo)
        let document_analysis =
            DocumentAnalysis::analyze_text_elements(&preprocessor_output.text_elements);

        // Stage 3: Rule processing with config (TextElements + Config → ParsedElements)
        let parsed_elements = if config.minimal_parse {
            println!("🔄 Minimal parse mode - skipping rule processing");
            self.rule_engine
                .convert_text_elements_to_parsed(&preprocessor_output.text_elements)
        } else {
            let font_size_analysis = self.rule_engine.analyze_font_sizes(
                &preprocessor_output.text_elements,
                &preprocessor_output.style_data,
            );

            // Apply rules with config guiding behavior
            self.rule_engine.apply_rules_with_config(
                &preprocessor_output.text_elements,
                &classification,
                &document_analysis,
                &font_size_analysis,
                &preprocessor_output.style_data,
                config, // Config flows through rule engine
            )?
        };

        println!(
            "⏱️  Rule processing: {:.3}s",
            stage3_start.elapsed().as_secs_f64()
        );

        let stage4_start = Instant::now();

        // Infer title from content before elements are consumed by graph builder
        let inferred_title = infer_title(&parsed_elements);

        // Stage 4: Graph building (ParsedElements + Config → Graph)
        let mut graph = self.graph_builder.build_graph(parsed_elements)?;
        println!(
            "⏱️  Graph construction: {:.3}s",
            stage4_start.elapsed().as_secs_f64()
        );

        // Stage 5: Wire metadata and compute post-processing
        if let Some(title) = inferred_title {
            graph.document_info.document_metadata.title = Some(title);
        }
        graph.document_info.document_metadata.merge_extracted(preprocessor_output.metadata);
        graph.document_info.document_analysis = document_analysis;
        graph.compute_structural_profile();
        graph.compute_breadcrumbs();

        Ok(graph)
    }

    /// Internal processing with detailed profiling
    fn process_with_config_flow_and_profiler(
        &mut self,
        input_path: &str,
        config: &ParsingConfig,
        profiler: &mut StepProfiler,
        pdf_hash: &str,
    ) -> Result<DocumentGraph> {
        // Stage 1: Preprocessing with sub-steps
        // Check Level 1 preprocessor cache first (keyed by PDF hash alone, config-independent)
        let preprocessor_output =
            if let Some(cached) = self.storage.get_preprocessor_output(pdf_hash)? {
                println!("🎯 Preprocessor cache hit — skipping Tika extraction");
                cached
            } else {
                let input_path = Path::new(input_path);
                let pdf_bytes = std::fs::read(input_path)?;
                let markup = profiler.time_step("1. PDF → Markup", || {
                    self.preprocessor.parse_pdf_to_markup_language(&pdf_bytes)
                })?;

                let output = profiler.time_step("2. Markup → TextElements", || {
                    self.preprocessor
                        .parse_markup_to_preprocessor_output(&markup)
                })?;

                // Store in preprocessor cache for future config sweeps
                self.storage
                    .store_preprocessor_output(pdf_hash, &output)?;
                output
            };

        // Stage 2: Classification
        let classification = profiler.time_step("3. Classification", || {
            self.classifier.classify(&preprocessor_output)
        })?;

        // Compute document analysis once (used by rules and stored in DocumentInfo)
        let document_analysis = profiler.time_step("4a. Document Analysis", || {
            DocumentAnalysis::analyze_text_elements(&preprocessor_output.text_elements)
        });

        // Stage 3: Rule processing with detailed timing
        let parsed_elements = if config.minimal_parse {
            profiler.time_step("4. Minimal Parse", || {
                self.rule_engine
                    .convert_text_elements_to_parsed(&preprocessor_output.text_elements)
            })
        } else {
            let font_size_analysis = profiler.time_step("4b. Font Analysis", || {
                self.rule_engine.analyze_font_sizes(
                    &preprocessor_output.text_elements,
                    &preprocessor_output.style_data,
                )
            });

            profiler.time_step("4c. Rules Processing", || {
                self.rule_engine.apply_rules_with_config(
                    &preprocessor_output.text_elements,
                    &classification,
                    &document_analysis,
                    &font_size_analysis,
                    &preprocessor_output.style_data,
                    config,
                )
            })?
        };

        // Infer title from content before elements are consumed by graph builder
        let inferred_title = infer_title(&parsed_elements);

        // Stage 4: Graph building
        let mut graph = profiler.time_step("5. Graph Construction", || {
            self.graph_builder.build_graph(parsed_elements)
        })?;

        // Stage 5: Wire metadata and compute post-processing
        if let Some(title) = inferred_title {
            graph.document_info.document_metadata.title = Some(title);
        }
        graph.document_info.document_metadata.merge_extracted(preprocessor_output.metadata);
        graph.document_info.document_analysis = document_analysis;
        graph.compute_structural_profile();
        graph.compute_breadcrumbs();

        Ok(graph)
    }

    /// Main document processing function with all options
    pub fn process_document_with_options(
        &mut self,
        input_path: &str,
        include_raw_tika: bool,
        output_dir: Option<&str>,
        debug_output: bool,
        debug_filters: &[String],
        minimal_parse: Option<bool>,
    ) -> Result<DocumentGraph> {
        let start_time = Instant::now();
        println!("📄 Processing document: {}", input_path);

        // Step 1: Use preprocessor to extract and parse document
        let preprocessor_output = if include_raw_tika || output_dir.is_some() {
            // For now, handle raw output options by doing two-step process manually
            let input_path = Path::new(input_path);
            let pdf_bytes = std::fs::read(input_path)?;
            let markup = self.preprocessor.parse_pdf_to_markup_language(&pdf_bytes)?;

            // Save raw markup if requested
            if include_raw_tika {
                if let Some(output_dir) = output_dir {
                    use std::fs;
                    let raw_path = format!("{}/raw_tika_output.html", output_dir);
                    if let Err(e) = fs::write(&raw_path, &markup) {
                        println!("⚠️  Failed to save raw markup to {}: {}", raw_path, e);
                    } else {
                        println!("💾 Saved raw markup to {}", raw_path);
                    }
                }
            }

            self.preprocessor
                .parse_markup_to_preprocessor_output(&markup)?
        } else {
            // Standard processing - use the convenience method
            let input_path = Path::new(input_path);
            self.preprocessor.process_file(input_path)?
        };

        println!(
            "⏱️  Preprocessing complete: {:.3}s",
            start_time.elapsed().as_secs_f64()
        );

        let step2_start = Instant::now();

        // Step 2: Document classification
        let classification = self.classifier.classify(&preprocessor_output)?;
        println!("📋 Document classified as: {:?}", classification);

        // Step 3: Get text elements (already parsed by preprocessor)
        println!(
            "⏱️  Text parsing: {:.3}s",
            step2_start.elapsed().as_secs_f64()
        );

        let step3_start = Instant::now();

        // Compute document analysis once (used by rules and stored in DocumentInfo)
        let document_analysis =
            DocumentAnalysis::analyze_text_elements(&preprocessor_output.text_elements);

        // Step 4: Apply rules (skip if minimal parse requested)
        let parsed_elements = if minimal_parse.unwrap_or(false) {
            println!("🔄 Minimal parse mode - skipping rule processing");
            // Convert text elements to parsed elements without processing
            self.rule_engine
                .convert_text_elements_to_parsed(&preprocessor_output.text_elements)
        } else {
            // Set up debug config
            if debug_output {
                let debug_config = DebugConfig {
                    enabled: true,
                    filter_patterns: debug_filters.to_vec(),
                };
                self.rule_engine.set_debug_config(debug_config);
            }

            let font_size_analysis = self.rule_engine.analyze_font_sizes(
                &preprocessor_output.text_elements,
                &preprocessor_output.style_data,
            );

            // Apply rules to get processed elements
            self.rule_engine.apply_rules(
                &preprocessor_output.text_elements,
                &classification,
                &document_analysis,
                &font_size_analysis,
                &preprocessor_output.style_data,
            )?
        };

        println!(
            "⏱️  Rule processing: {:.3}s",
            step3_start.elapsed().as_secs_f64()
        );

        let step4_start = Instant::now();

        // Infer title from content before elements are consumed by graph builder
        let inferred_title = infer_title(&parsed_elements);

        // Step 5: Build graph from processed elements
        let mut graph = self.graph_builder.build_graph(parsed_elements)?;

        // Step 6: Wire metadata and compute post-processing
        if let Some(title) = inferred_title {
            graph.document_info.document_metadata.title = Some(title);
        }
        graph.document_info.document_metadata.merge_extracted(preprocessor_output.metadata);
        graph.document_info.document_analysis = document_analysis;
        graph.compute_structural_profile();
        graph.compute_breadcrumbs();

        println!(
            "⏱️  Graph construction: {:.3}s",
            step4_start.elapsed().as_secs_f64()
        );
        println!(
            "⏱️  Total processing time: {:.3}s",
            start_time.elapsed().as_secs_f64()
        );

        Ok(graph)
    }

    /// Process document and capture all intermediate stage outputs
    /// Used for pipeline diagnostics and testing stage boundaries
    pub fn process_document_capture_stages(
        &mut self,
        input_path: &str,
        config: &ParsingConfig,
    ) -> Result<PipelineStages> {
        let input_path_ref = Path::new(input_path);
        let pdf_bytes = std::fs::read(input_path_ref)?;

        // Stage 1a: PDF → XHTML
        let xhtml = self.preprocessor.parse_pdf_to_markup_language(&pdf_bytes)?;
        println!("📋 Stage 1a: XHTML captured ({} bytes)", xhtml.len());

        // Stage 1b: XHTML → TextElements
        let preprocessor_output = self
            .preprocessor
            .parse_markup_to_preprocessor_output(&xhtml)?;
        let text_elements = preprocessor_output.text_elements.clone();
        println!("📋 Stage 1b: {} TextElements captured", text_elements.len());

        // Stage 2: Classification + Rules → ParsedElements
        let classification = self.classifier.classify(&preprocessor_output)?;
        let document_analysis =
            DocumentAnalysis::analyze_text_elements(&preprocessor_output.text_elements);

        let parsed_elements = if config.minimal_parse {
            self.rule_engine
                .convert_text_elements_to_parsed(&preprocessor_output.text_elements)
        } else {
            let font_size_analysis = self.rule_engine.analyze_font_sizes(
                &preprocessor_output.text_elements,
                &preprocessor_output.style_data,
            );
            self.rule_engine.apply_rules_with_config(
                &preprocessor_output.text_elements,
                &classification,
                &document_analysis,
                &font_size_analysis,
                &preprocessor_output.style_data,
                config,
            )?
        };
        println!(
            "📋 Stage 2: {} ParsedElements captured",
            parsed_elements.len()
        );

        // Infer title from content before graph build
        let inferred_title = infer_title(&parsed_elements);

        // Stage 3: ParsedElements → DocumentGraph
        let mut graph = self.graph_builder.build_graph(parsed_elements.clone())?;

        // Wire metadata and compute post-processing
        if let Some(title) = inferred_title {
            graph.document_info.document_metadata.title = Some(title);
        }
        graph.document_info.document_metadata.merge_extracted(preprocessor_output.metadata);
        graph.document_info.document_analysis = document_analysis;
        graph.compute_structural_profile();
        graph.compute_breadcrumbs();

        println!(
            "📋 Stage 3: Graph captured ({} nodes)",
            graph.nodes.len()
        );

        Ok(PipelineStages {
            xhtml,
            text_elements,
            parsed_elements,
            graph,
        })
    }

    /// Simple document processing function using default config
    pub fn process_document(&mut self, input_path: &str) -> Result<DocumentGraph> {
        let default_config = ParsingConfig::default();
        self.process_document_with_config(input_path, &default_config)
    }

    /// Process document with config loaded from file
    pub fn process_document_with_config_file(
        &mut self,
        input_path: &str,
        config_path: &str,
    ) -> Result<DocumentGraph> {
        let config = ParsingConfig::load_from_file(config_path)?;
        self.process_document_with_config(input_path, &config)
    }
}

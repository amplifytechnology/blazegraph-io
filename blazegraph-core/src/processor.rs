use crate::cache::{GraphCacheKey, GraphCacheValue};
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

    /// Future: Convenience constructor for API usage (server Tika + database storage)
    /// This will be implemented when server-based Tika preprocessor is available
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

        // Calculate config hash for Level 2 cache
        let config_hash = calculate_config_hash(config)?;
        let cache_key = GraphCacheKey::new(pdf_hash.clone(), config_hash);

        // Check Level 2 cache: Config + PDF → Graph
        if let Some(cached) = self.storage.get_graph_output(&cache_key)? {
            println!("🎯 Cache hit: Found graph for PDF + config combination");
            println!(
                "⏱️  Total processing time: {:.3}s (cached)",
                start_time.elapsed().as_secs_f64()
            );
            return Ok(cached.graph);
        }

        println!("📄 Processing document with config: {}", input_path);

        // Process with config flow
        let graph = self.process_with_config_flow(input_path, config)?;

        // Store in Level 2 cache
        let processing_time = start_time.elapsed().as_millis() as u64;
        let cache_value = GraphCacheValue::new(graph.clone(), processing_time);
        self.storage.store_graph_output(&cache_key, &cache_value)?;

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
        skip_cache: bool,
    ) -> Result<DocumentGraph> {
        let start_time = Instant::now();

        // Check cache first (timed)
        let (_pdf_hash, cache_key) = profiler.time_step("Cache Key Generation", || {
            let pdf_bytes = std::fs::read(input_path)?;
            let pdf_hash = calculate_pdf_hash(&pdf_bytes);
            let config_hash = calculate_config_hash(config)?;
            let cache_key = GraphCacheKey::new(pdf_hash.clone(), config_hash);
            Ok::<(String, GraphCacheKey), anyhow::Error>((pdf_hash, cache_key))
        })?;

        let cached_result = if skip_cache {
            println!("🚫 Skipping cache lookup (--skip-cache enabled)");
            None
        } else {
            profiler.time_step("Cache Lookup", || self.storage.get_graph_output(&cache_key))?
        };

        if let Some(cached) = cached_result {
            println!("🎯 Cache hit: Found graph for PDF + config combination");
            profiler.print_summary();
            println!(
                "⏱️  Total processing time: {:.0}ms (cached)",
                start_time.elapsed().as_millis()
            );
            return Ok(cached.graph);
        }

        println!("📄 Processing document with config: {}", input_path);

        // Process with detailed profiling
        let graph =
            self.process_with_config_flow_and_profiler(input_path, config, &mut profiler)?;

        // Store in cache (timed) unless skipping cache
        if !skip_cache {
            profiler.time_step("Cache Storage", || {
                let processing_time = start_time.elapsed().as_millis() as u64;
                let cache_value = GraphCacheValue::new(graph.clone(), processing_time);
                self.storage.store_graph_output(&cache_key, &cache_value)
            })?;
        } else {
            println!("🚫 Skipping cache storage (--skip-cache enabled)");
        }

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
    ) -> Result<DocumentGraph> {
        let stage1_start = Instant::now();

        // Stage 1: Preprocessing (PDF → TextElements)
        let input_path = Path::new(input_path);
        let preprocessor_output = self.preprocessor.process_file(input_path)?;
        println!(
            "⏱️  Preprocessing: {:.3}s",
            stage1_start.elapsed().as_secs_f64()
        );

        let stage2_start = Instant::now();

        // Stage 2: Classification
        let classification = self.classifier.classify(&preprocessor_output)?;
        println!("📋 Document classified as: {:?}", classification);
        println!(
            "⏱️  Classification: {:.3}s",
            stage2_start.elapsed().as_secs_f64()
        );

        let stage3_start = Instant::now();

        // Stage 3: Rule processing with config (TextElements + Config → ParsedElements)
        let parsed_elements = if config.minimal_parse {
            println!("🔄 Minimal parse mode - skipping rule processing");
            self.rule_engine
                .convert_text_elements_to_parsed(&preprocessor_output.text_elements)
        } else {
            // Analyze document for patterns
            let document_analysis =
                DocumentAnalysis::analyze_text_elements(&preprocessor_output.text_elements);
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

        // Stage 4: Graph building (ParsedElements + Config → Graph)
        let graph = self.graph_builder.build_graph(parsed_elements)?;
        println!(
            "⏱️  Graph construction: {:.3}s",
            stage4_start.elapsed().as_secs_f64()
        );

        Ok(graph)
    }

    /// Internal processing with detailed profiling
    fn process_with_config_flow_and_profiler(
        &mut self,
        input_path: &str,
        config: &ParsingConfig,
        profiler: &mut StepProfiler,
    ) -> Result<DocumentGraph> {
        // Stage 1: Preprocessing with sub-steps
        let input_path = Path::new(input_path);
        let pdf_bytes = std::fs::read(input_path)?;
        let markup = profiler.time_step("1. PDF → Markup", || {
            self.preprocessor.parse_pdf_to_markup_language(&pdf_bytes)
        })?;

        let preprocessor_output = profiler.time_step("2. Markup → TextElements", || {
            self.preprocessor
                .parse_markup_to_preprocessor_output(&markup)
        })?;

        // Stage 2: Classification
        let classification = profiler.time_step("3. Classification", || {
            self.classifier.classify(&preprocessor_output)
        })?;

        // Stage 3: Rule processing with detailed timing
        let parsed_elements = if config.minimal_parse {
            profiler.time_step("4. Minimal Parse", || {
                self.rule_engine
                    .convert_text_elements_to_parsed(&preprocessor_output.text_elements)
            })
        } else {
            let document_analysis = profiler.time_step("4a. Document Analysis", || {
                DocumentAnalysis::analyze_text_elements(&preprocessor_output.text_elements)
            });

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

        // Stage 4: Graph building
        let graph = profiler.time_step("5. Graph Construction", || {
            self.graph_builder.build_graph(parsed_elements)
        })?;

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

            // Analyze document for font size patterns
            let document_analysis =
                DocumentAnalysis::analyze_text_elements(&preprocessor_output.text_elements);
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

        // Step 5: Build graph from processed elements
        let graph = self.graph_builder.build_graph(parsed_elements)?;

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
        let parsed_elements = if config.minimal_parse {
            self.rule_engine
                .convert_text_elements_to_parsed(&preprocessor_output.text_elements)
        } else {
            let document_analysis =
                DocumentAnalysis::analyze_text_elements(&preprocessor_output.text_elements);
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

        // Stage 3: ParsedElements → DocumentGraph
        let graph = self.graph_builder.build_graph(parsed_elements.clone())?;
        println!(
            "📋 Stage 3: Graph captured ({} nodes, {} edges)",
            graph.nodes.len(),
            graph.edges.len()
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

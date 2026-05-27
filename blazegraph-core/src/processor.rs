use crate::analytics::{
    AnalysisBuilder, DocumentAnalysis, FontStatsBuilder, GeometryStatsBuilder, PageStatsBuilder,
    RegionStatsBuilder, Statistic,
};
use crate::cache::GraphCacheKey;
use crate::classifier::DocumentClassifier;
use crate::config::ParsingConfig;
use crate::graphs::builder::GraphBuilder;
use crate::graphs::NodeIdGenerator;
use crate::preprocessors::pdf::project_to_semantic_tree;
use crate::preprocessors::{Preprocessor, TikaPreprocessor};
use crate::rules::RuleEngine;
use crate::storage::{
    calculate_config_hash, calculate_source_hash, CacheDefaults, CachePoint, DocumentStorage,
    FileStorage, FreshFrom,
};
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

    // =========================================================================
    // Main entry points
    // =========================================================================

    /// Process document with cache point awareness (CR-11).
    /// This is the primary entry point for CLI usage.
    pub fn process_document_with_cache(
        &mut self,
        input_path: &str,
        config: &ParsingConfig,
        fresh_from: FreshFrom,
        cache_defaults: &CacheDefaults,
        enable_profiling: bool,
    ) -> Result<DocumentGraph> {
        let mut profiler = StepProfiler::new(enable_profiling);
        let start_time = Instant::now();

        // Read PDF and calculate hash
        let pdf_bytes = std::fs::read(input_path)?;
        let pdf_hash = calculate_source_hash(&pdf_bytes);

        println!("📄 Processing: {}", input_path);

        // --- C3: Graph cache check ---
        if fresh_from.should_use_cache(CachePoint::C3)
            && cache_defaults.should_write(CachePoint::C3)
        {
            let config_hash = calculate_config_hash(config)?;
            let cache_key = GraphCacheKey::new(pdf_hash.clone(), config_hash);
            if let Some(cached) = self.storage.get_graph_output(&cache_key)? {
                println!(
                    "🎯 C3 graph cache hit ({:.3}s)",
                    start_time.elapsed().as_secs_f64()
                );
                return Ok(cached.graph);
            }
        }

        // Build the provenance record that identifies this parse run.
        // The `(source_sha256, config_hash)` pair feeds
        // `NodeIdGenerator::new` (CR-47: node IDs depend on source +
        // config only — `blazegraph_version` no longer enters the
        // namespace, so node IDs survive parser version bumps).
        // `parse_provenance` is persisted on the graph so the bgraph.md
        // emitter (B2) can populate the document-level identity block
        // without re-reading the source. `blazegraph_version` rides
        // along as provenance documentation only.
        let config_hash = calculate_config_hash(config)?;
        let provenance = ParseProvenance {
            blazegraph_version: env!("CARGO_PKG_VERSION").to_string(),
            source_format: "pdf".to_string(),
            source_filename: Path::new(input_path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| input_path.to_string()),
            source_sha256: pdf_hash.clone(),
            config_hash: config_hash.clone(),
        };
        let id_gen = NodeIdGenerator::new(&provenance.source_sha256, &provenance.config_hash);

        // --- C2: Preprocessor cache check ---
        let preprocessor_output = if fresh_from.should_use_cache(CachePoint::C2) {
            if let Some(cached) = self.storage.get_preprocessor_output(&pdf_hash)? {
                println!("🎯 C2 preprocessor cache hit — skipping extraction + parsing");
                cached
            } else {
                self.extract_and_parse(
                    input_path,
                    &pdf_bytes,
                    &pdf_hash,
                    &fresh_from,
                    cache_defaults,
                    &mut profiler,
                )?
            }
        } else {
            self.extract_and_parse(
                input_path,
                &pdf_bytes,
                &pdf_hash,
                &fresh_from,
                cache_defaults,
                &mut profiler,
            )?
        };

        // --- Stages 2-5: Classification → Rules → Graph → Post-processing ---
        let graph = self.rules_and_graph(
            &preprocessor_output,
            config,
            &id_gen,
            provenance,
            &pdf_hash,
            &mut profiler,
        )?;

        if enable_profiling {
            profiler.print_summary();
        }
        println!("⏱️  Total: {:.0}ms", start_time.elapsed().as_millis());

        Ok(graph)
    }

    /// Simple document processing function using default config (no cache awareness)
    pub fn process_document(&mut self, input_path: &str) -> Result<DocumentGraph> {
        let default_config = ParsingConfig::default();
        self.process_document_with_cache(
            input_path,
            &default_config,
            FreshFrom::None,
            &CacheDefaults::default(),
            false,
        )
    }

    /// Process document with config loaded from file
    pub fn process_document_with_config_file(
        &mut self,
        input_path: &str,
        config_path: &str,
    ) -> Result<DocumentGraph> {
        let config = ParsingConfig::load_from_file(config_path)?;
        self.process_document_with_cache(
            input_path,
            &config,
            FreshFrom::None,
            &CacheDefaults::default(),
            false,
        )
    }

    /// Process document and capture all intermediate stage outputs
    /// Used for pipeline diagnostics (--dump-stages). Always runs fresh.
    pub fn process_document_capture_stages(
        &mut self,
        input_path: &str,
        config: &ParsingConfig,
    ) -> Result<PipelineStages> {
        let input_path_ref = Path::new(input_path);
        let pdf_bytes = std::fs::read(input_path_ref)?;
        let pdf_hash = calculate_source_hash(&pdf_bytes);

        // Stage 1a: PDF → XHTML (always fresh for diagnostics)
        let xhtml = self.preprocessor.parse_pdf_to_markup_language(&pdf_bytes)?;
        println!("📋 Stage 1a: XHTML captured ({} bytes)", xhtml.len());

        // Stage 1b: XHTML → TextElements
        let preprocessor_output = self
            .preprocessor
            .parse_markup_to_preprocessor_output(&xhtml)?;
        println!(
            "📋 Stage 1b: {} TextElements captured",
            preprocessor_output.text_elements.len()
        );

        // Stage 2: Classification + Rules → ParsedElements
        let classification = self.classifier.classify(&preprocessor_output)?;
        let document_analysis = run_analytics(&preprocessor_output.text_elements);
        if config.dump_analytics {
            dump_stats(&*self.storage, &pdf_hash, &document_analysis)?;
        }

        // Reading-order resort + region tagging (Block 06b). Capture the
        // post-resort stream as the canonical Stage 1b snapshot — this is
        // the version that flows into rules and carries `region_label`.
        let text_elements = crate::analytics::tag_and_resort(
            preprocessor_output.text_elements.clone(),
            &document_analysis,
        );
        let resorted_elements = text_elements.clone();

        let parsed_elements = if config.minimal_parse {
            self.rule_engine
                .convert_text_elements_to_parsed(&resorted_elements)
        } else {
            let font_size_analysis = self
                .rule_engine
                .analyze_font_sizes(&resorted_elements, &preprocessor_output.style_data);
            self.rule_engine.apply_rules_with_config(
                &resorted_elements,
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

        // Stage 3: ParsedPdfElement → SemanticTreeElement (channel exit)
        let semantic_elements = project_to_semantic_tree(parsed_elements.clone());

        // Stage 4: SemanticTreeElement → DocumentGraph
        let mut graph = self.graph_builder.build_graph(semantic_elements)?;

        // Wire metadata and compute post-processing. CR-57: each channel
        // now writes a complete DocumentMetadata in its extractor — direct
        // assignment replaces the old merge_extracted semantic.
        graph.document_info.document_metadata = preprocessor_output.metadata;
        // Body-side title inference is honored only when source-native
        // extraction returned None. F-02 (title-cleanup) is deferred to
        // the composition layer per `09-metadata-first-class.md`; the
        // existing PDF pipeline still relies on the inferred fallback
        // so we preserve it explicitly until the composition layer lands.
        if graph.document_info.document_metadata.title.is_none() {
            if let Some(title) = inferred_title {
                graph.document_info.document_metadata.title = Some(title);
            }
        }
        graph.document_info.bookmark_data = preprocessor_output.bookmark_data;
        // CR-66 ordering: structural profile is computed twice — once
        // before graph_sanity (so invariant checks can reason from a
        // current node-type / depth-distribution view) and once after
        // (so consumers see post-mutation counts). Breadcrumbs run only
        // after, since they're a derived output, not a sanity input.
        graph.compute_structural_profile();
        crate::graphs::graph_sanity::apply(&mut graph, &config.graph_sanity);
        graph.compute_structural_profile();
        graph.compute_breadcrumbs();

        println!("📋 Stage 3: Graph captured ({} nodes)", graph.nodes.len());

        Ok(PipelineStages {
            xhtml,
            text_elements,
            parsed_elements,
            graph,
        })
    }

    // =========================================================================
    // Internal: extraction + parsing with C1/C2 cache awareness
    // =========================================================================

    /// Extract XHTML and parse to PreprocessorOutput, respecting C1 and C2 caches.
    fn extract_and_parse(
        &mut self,
        _input_path: &str,
        pdf_bytes: &[u8],
        pdf_hash: &str,
        fresh_from: &FreshFrom,
        cache_defaults: &CacheDefaults,
        profiler: &mut StepProfiler,
    ) -> Result<PreprocessorOutput> {
        // --- C1: XHTML cache check ---
        let xhtml = if fresh_from.should_use_cache(CachePoint::C1) {
            if let Some(cached) = self.storage.get_xhtml(pdf_hash)? {
                println!("🎯 C1 XHTML cache hit — skipping Tika extraction");
                cached
            } else {
                let markup = profiler.time_step("C1: PDF → XHTML (Tika)", || {
                    self.preprocessor.parse_pdf_to_markup_language(pdf_bytes)
                })?;
                if cache_defaults.should_write(CachePoint::C1) {
                    self.storage.store_xhtml(pdf_hash, &markup)?;
                    println!("💾 C1: XHTML cached ({} bytes)", markup.len());
                }
                markup
            }
        } else {
            // Fresh extraction requested
            let markup = profiler.time_step("C1: PDF → XHTML (Tika, fresh)", || {
                self.preprocessor.parse_pdf_to_markup_language(pdf_bytes)
            })?;
            if cache_defaults.should_write(CachePoint::C1) {
                self.storage.store_xhtml(pdf_hash, &markup)?;
                println!("💾 C1: XHTML cached ({} bytes, refreshed)", markup.len());
            }
            markup
        };

        // --- C2: Parse XHTML → PreprocessorOutput ---
        let output = profiler.time_step("C2: XHTML → PreprocessorOutput", || {
            self.preprocessor
                .parse_markup_to_preprocessor_output(&xhtml)
        })?;

        if cache_defaults.should_write(CachePoint::C2) {
            self.storage.store_preprocessor_output(pdf_hash, &output)?;
            println!("💾 C2: PreprocessorOutput cached");
        }

        Ok(output)
    }

    // =========================================================================
    // Internal: classification → rules → graph (shared by all entry points)
    // =========================================================================

    /// Run classification, rules, and graph building on PreprocessorOutput.
    fn rules_and_graph(
        &mut self,
        preprocessor_output: &PreprocessorOutput,
        config: &ParsingConfig,
        id_gen: &NodeIdGenerator,
        parse_provenance: ParseProvenance,
        pdf_hash: &str,
        profiler: &mut StepProfiler,
    ) -> Result<DocumentGraph> {
        // Classification
        let classification = profiler.time_step("Classification", || {
            self.classifier.classify(preprocessor_output)
        })?;

        // Document analytics pre-pass (read by rules; sidecar-dumped to
        // `{cache_dir}/stat/<name>/<pdf_hash>.json` when `config.dump_analytics`).
        // No longer persisted into graph.json — that field went away with schema 0.4.0.
        let document_analysis = profiler.time_step("Document Analytics", || {
            run_analytics(&preprocessor_output.text_elements)
        });
        if config.dump_analytics {
            dump_stats(&*self.storage, pdf_hash, &document_analysis)?;
        }

        // Reading-order resort + region tagging (Block 06b). Annotates each
        // element with its Region tree leaf label and reorders the stream so
        // multi-column pages no longer interleave columns. Owned-clone of
        // `text_elements` because PreprocessorOutput is borrowed immutably
        // here; the cost is one Vec clone per document, negligible vs the
        // rules / graph-build work that follows.
        let text_elements = profiler.time_step("Reading-Order Resort", || {
            crate::analytics::tag_and_resort(
                preprocessor_output.text_elements.clone(),
                &document_analysis,
            )
        });

        // Rule processing
        let parsed_elements = if config.minimal_parse {
            println!("🔄 Minimal parse mode — skipping rule processing");
            self.rule_engine
                .convert_text_elements_to_parsed(&text_elements)
        } else {
            let font_size_analysis = profiler.time_step("Font Analysis", || {
                self.rule_engine
                    .analyze_font_sizes(&text_elements, &preprocessor_output.style_data)
            });

            profiler.time_step("Rules Processing", || {
                self.rule_engine.apply_rules_with_config(
                    &text_elements,
                    &classification,
                    &document_analysis,
                    &font_size_analysis,
                    &preprocessor_output.style_data,
                    config,
                )
            })?
        };

        // Infer title before graph build consumes elements
        let inferred_title = infer_title(&parsed_elements);

        // PDF channel exit: project rule output onto SemanticTreeElement.
        // Everything from here is channel-agnostic.
        let semantic_elements = profiler.time_step("Channel Projection", || {
            project_to_semantic_tree(parsed_elements)
        });

        // Graph construction (deterministic UUIDv5 node IDs)
        let mut graph = profiler.time_step("Graph Construction", || {
            self.graph_builder.build_graph_deterministic(
                semantic_elements,
                id_gen,
                parse_provenance,
            )
        })?;

        // Post-processing: metadata, analysis, breadcrumbs. CR-57: direct
        // assignment replaces the old merge_extracted call (each channel
        // now writes a complete DocumentMetadata in its extractor).
        graph.document_info.document_metadata = preprocessor_output.metadata.clone();
        // Body-side title inference is honored only when source-native
        // extraction returned None — see the entry-point analog above
        // for the F-02 deferral rationale.
        if graph.document_info.document_metadata.title.is_none() {
            if let Some(title) = inferred_title {
                graph.document_info.document_metadata.title = Some(title);
            }
        }
        graph.document_info.bookmark_data = preprocessor_output.bookmark_data.clone();
        // CR-66 ordering: structural profile is computed twice — once
        // before graph_sanity (so invariant checks can reason from a
        // current node-type / depth-distribution view) and once after
        // (so consumers see post-mutation counts). Breadcrumbs run only
        // after, since they're a derived output, not a sanity input.
        graph.compute_structural_profile();
        crate::graphs::graph_sanity::apply(&mut graph, &config.graph_sanity);
        graph.compute_structural_profile();
        graph.compute_breadcrumbs();

        Ok(graph)
    }
}

/// Run the document-analytics pre-pass over a slice of text elements.
///
/// Single-pass walk: dispatches each element to every enabled stat kind via
/// `AnalysisBuilder`, then finalizes in dependency order. Output is consumed
/// in pipeline memory by downstream rules and (when `dump_analytics`) written
/// to per-stat sidecar files via [`dump_stats`].
fn run_analytics(text_elements: &[PdfTextElement]) -> DocumentAnalysis {
    let mut builder = AnalysisBuilder::new();
    for element in text_elements {
        builder.observe(element);
    }
    builder.finalize()
}

/// Per-stat sidecar dump. One JSON file per stat kind under
/// `{cache_dir}/stat/<Statistic::NAME>/<pdf_hash>.json`. Folder-per-stat
/// scoping (Marcus, Block 05) lets future stat kinds (RegionStats,
/// PageOutlier, …) drop in without colliding. The full composite is the
/// in-memory shape; the sidecar splits it for grep-ability and per-stat diff
/// against Python prototype outputs.
fn dump_stats(
    storage: &dyn DocumentStorage,
    pdf_hash: &str,
    analysis: &DocumentAnalysis,
) -> Result<()> {
    let font_json = serde_json::to_string_pretty(&analysis.font)?;
    storage.store_stat(pdf_hash, FontStatsBuilder::NAME, &font_json)?;

    let geometry_json = serde_json::to_string_pretty(&analysis.geometry)?;
    storage.store_stat(pdf_hash, GeometryStatsBuilder::NAME, &geometry_json)?;

    let page_stats_json = serde_json::to_string_pretty(&analysis.page_stats)?;
    storage.store_stat(pdf_hash, PageStatsBuilder::NAME, &page_stats_json)?;

    let region_json = serde_json::to_string_pretty(&analysis.region)?;
    storage.store_stat(pdf_hash, RegionStatsBuilder::NAME, &region_json)?;

    Ok(())
}

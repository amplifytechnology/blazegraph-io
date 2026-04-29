use super::*;

#[test]
fn analysis_builder_compiles_and_finalizes_to_defaults() {
    let builder = AnalysisBuilder::new();
    let analysis = builder.finalize();
    // Default outputs round-trip through serde without panic.
    let json = serde_json::to_string(&analysis).expect("serialize");
    let parsed: super::DocumentAnalysis = serde_json::from_str(&json).expect("deserialize");
    let _ = parsed;
}

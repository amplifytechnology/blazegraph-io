use std::path::Path;

fn main() {
    // Rerun if JAR files change
    println!("cargo:rerun-if-changed=../blazegraph-core/deps/tika/jni-jars/");
    println!("cargo:rerun-if-changed=src/tika/jars/");
    
    // Check for Tika JAR (used by JNI backend)
    // Primary location: core deps (shared dependency)
    let core_jar_path = "../blazegraph-core/deps/tika/jni-jars/blazing-tika-jni.jar";
    // Legacy location: CLI-specific (for backwards compatibility)
    let legacy_jar_path = "src/tika/jars/blazing-tika-jni.jar";
    
    if Path::new(core_jar_path).exists() {
        println!("cargo:rustc-cfg=has_custom_tika_jar");
        println!("cargo:warning=Found Tika JAR at: {}", core_jar_path);
    } else if Path::new(legacy_jar_path).exists() {
        println!("cargo:rustc-cfg=has_custom_tika_jar");
        println!("cargo:warning=Found Tika JAR at legacy location: {}", legacy_jar_path);
        println!("cargo:warning=Consider moving to: {}", core_jar_path);
    } else {
        println!("cargo:warning=Tika JAR not found!");
        println!("cargo:warning=Expected at: {}", core_jar_path);
        println!("cargo:warning=JNI backend will require JAR path to be specified");
    }
}

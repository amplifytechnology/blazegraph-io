//! JNI Backend for PDF processing via Apache Tika
//!
//! Uses JNI to call into a bundled JRE running our custom Tika processor.
//! This is the primary backend for cross-platform deployments.
//!
//! # Resource Management
//! The caller (CLI, API) is responsible for providing valid paths to:
//! - JRE directory (must contain lib/server/libjvm.so or equivalent)
//! - JAR file (blazing-tika.jar with TikaMain class)
//!
//! # JVM Lifecycle
//! Only ONE JVM can exist per process. The JVM is created on first instantiation
//! and lives for the lifetime of the process.

use super::PdfBackend;
use anyhow::{anyhow, Result};
use jni::{InitArgsBuilder, JNIVersion, JavaVM};
use std::path::Path;
use std::sync::Arc;

/// JNI-based Tika backend for PDF processing
///
/// Uses JNI to call TikaMain.processToXhtml(byte[]) in the bundled JRE.
///
/// # Memory Model
/// - **Rust heap**: Input PDF bytes, output String (managed by Rust ownership)
/// - **Java heap**: Copied PDF bytes, Tika objects, result String (managed by Java GC)
/// - **Data flow**: Rust bytes → copy to Java → process → copy result to Rust
/// - JNI local references are auto-released after each call
///
/// # JVM Lifecycle
/// The JVM is created once and lives for the process lifetime. On normal drop,
/// the JVM runs its shutdown sequence (finalizers, GC) which can be slow.
/// For CLI tools, call `leak_for_fast_exit()` to skip this - the OS will
/// reclaim memory instantly when the process exits.
pub struct TikaJniBackend {
    jvm: Arc<JavaVM>,
    _jar_path: std::path::PathBuf,
}

// JNI works correctly across threads when properly attached
unsafe impl Send for TikaJniBackend {}
unsafe impl Sync for TikaJniBackend {}

impl TikaJniBackend {
    /// Create JNI backend with caller-provided paths and default JVM settings
    ///
    /// # Arguments
    /// * `jre_path` - Path to JRE directory (contains bin/java, lib/, etc.)
    /// * `jar_path` - Path to blazing-tika.jar
    ///
    /// Uses default JVM settings: 512MB heap, headless mode.
    /// For production, use `new_with_args()` to customize JVM settings.
    pub fn new(jre_path: &Path, jar_path: &Path) -> Result<Self> {
        Self::new_with_args(jre_path, jar_path, &[])
    }

    /// Create JNI backend with custom JVM arguments
    ///
    /// # Arguments
    /// * `jre_path` - Path to JRE directory (contains bin/java, lib/, etc.)
    /// * `jar_path` - Path to blazing-tika.jar
    /// * `extra_jvm_args` - Additional JVM arguments (e.g., "-Xmx4g", "-XX:+UseG1GC")
    ///
    /// # Resource Management
    /// Core does NOT manage these resources. Caller is responsible for:
    /// - Downloading/bundling JRE (CLI: download on first run, Docker: bundle)
    /// - Building/bundling JAR
    /// - Providing valid paths
    ///
    /// # JVM Lifecycle
    /// Only ONE JVM can exist per process. This constructor will fail if
    /// a JVM already exists.
    ///
    /// # Default JVM Arguments
    /// The following are always set:
    /// - `-Djava.class.path=<jar_path>`
    /// - `-Djava.awt.headless=true`
    ///
    /// If no heap arguments are provided in `extra_jvm_args`, defaults to:
    /// - `-Xms512m`
    /// - `-Xmx512m`
    pub fn new_with_args(jre_path: &Path, jar_path: &Path, extra_jvm_args: &[String]) -> Result<Self> {
        // Validate paths exist
        if !jre_path.exists() {
            return Err(anyhow!("JRE not found at: {}", jre_path.display()));
        }
        if !jar_path.exists() {
            return Err(anyhow!("JAR not found at: {}", jar_path.display()));
        }

        println!("🚀 TikaJniBackend initializing...");
        println!("   JRE path: {}", jre_path.display());
        println!("   JAR path: {}", jar_path.display());

        // Find libjvm
        let libjvm_path = Self::find_libjvm(jre_path)?;
        println!("   Found libjvm at: {}", libjvm_path.display());

        // CRITICAL: Set JAVA_HOME so the jni crate's java-locator can find the JVM
        // This must be done before calling JavaVM::new()
        std::env::set_var("JAVA_HOME", jre_path);

        // Set library path for JVM to find its native libraries
        Self::setup_library_path(jre_path)?;

        // Build JVM arguments
        let classpath = format!("-Djava.class.path={}", jar_path.display());

        let mut jvm_args_builder = InitArgsBuilder::new()
            .version(JNIVersion::V8)
            .option(&classpath)
            .option("-Djava.awt.headless=true");

        // Check if heap args are provided
        let has_heap_min = extra_jvm_args.iter().any(|arg| arg.starts_with("-Xms"));
        let has_heap_max = extra_jvm_args.iter().any(|arg| arg.starts_with("-Xmx"));

        // Add default heap if not specified
        if !has_heap_min {
            jvm_args_builder = jvm_args_builder.option("-Xms512m");
        }
        if !has_heap_max {
            jvm_args_builder = jvm_args_builder.option("-Xmx512m");
        }

        // Add extra JVM args
        for arg in extra_jvm_args {
            println!("   JVM arg: {}", arg);
            jvm_args_builder = jvm_args_builder.option(arg);
        }

        let jvm_args = jvm_args_builder
            .build()
            .map_err(|e| anyhow!("Failed to build JVM args: {:?}", e))?;

        // Create JVM (only one allowed per process)
        let jvm =
            JavaVM::new(jvm_args).map_err(|e| anyhow!("Failed to create JVM: {:?}", e))?;

        println!("✅ JVM created successfully");

        Ok(Self {
            jvm: Arc::new(jvm),
            _jar_path: jar_path.to_path_buf(),
        })
    }

    /// Leak the JVM to skip slow shutdown sequence
    ///
    /// Call this before process exit for instant termination.
    /// The OS will reclaim all memory anyway - no need for JVM's
    /// graceful shutdown (finalizers, GC) in CLI contexts.
    ///
    /// # Safety
    /// This is safe because:
    /// - The process is about to exit
    /// - OS will reclaim all memory
    /// - No resources need explicit cleanup (files are flushed, etc.)
    pub fn leak_for_fast_exit(self) {
        // Prevent Drop from running on the Arc<JavaVM>
        // This skips DestroyJavaVM() which runs finalizers and GC
        std::mem::forget(self.jvm);
    }

    /// Find libjvm.so/.dylib within JRE directory
    fn find_libjvm(jre_path: &Path) -> Result<std::path::PathBuf> {
        // Platform-specific library name and location
        #[cfg(target_os = "macos")]
        let candidates = vec![
            jre_path.join("lib/server/libjvm.dylib"),
            jre_path.join("lib/libjvm.dylib"),
        ];

        #[cfg(target_os = "linux")]
        let candidates = vec![
            jre_path.join("lib/server/libjvm.so"),
            jre_path.join("lib/libjvm.so"),
        ];

        #[cfg(target_os = "windows")]
        let candidates = vec![
            jre_path.join("bin/server/jvm.dll"),
            jre_path.join("bin/jvm.dll"),
        ];

        candidates
            .into_iter()
            .find(|p| p.exists())
            .ok_or_else(|| anyhow!("Could not find libjvm in JRE at {}", jre_path.display()))
    }

    /// Setup library path environment for JVM native library loading
    fn setup_library_path(jre_path: &Path) -> Result<()> {
        let lib_path = jre_path.join("lib");
        let server_path = jre_path.join("lib/server");

        #[cfg(target_os = "macos")]
        {
            let current = std::env::var("DYLD_LIBRARY_PATH").unwrap_or_default();
            let new_path = format!(
                "{}:{}:{}",
                server_path.display(),
                lib_path.display(),
                current
            );
            std::env::set_var("DYLD_LIBRARY_PATH", new_path);
        }

        #[cfg(target_os = "linux")]
        {
            let current = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
            let new_path = format!(
                "{}:{}:{}",
                server_path.display(),
                lib_path.display(),
                current
            );
            std::env::set_var("LD_LIBRARY_PATH", new_path);
        }

        #[cfg(target_os = "windows")]
        {
            let current = std::env::var("PATH").unwrap_or_default();
            let new_path = format!(
                "{};{};{}",
                server_path.display(),
                lib_path.display(),
                current
            );
            std::env::set_var("PATH", new_path);
        }

        Ok(())
    }
}

impl PdfBackend for TikaJniBackend {
    /// Process PDF bytes to Blazegraph XHTML
    ///
    /// # Thread Safety
    /// This method can be called from any thread. It will:
    /// 1. Attach the current thread to the JVM (if not already attached)
    /// 2. Call the Java method
    /// 3. The thread remains attached for future calls
    ///
    /// # Memory
    /// - Input bytes are copied to Java heap as byte[]
    /// - Output string is copied from Java heap to Rust
    /// - Java GC handles cleanup of Java objects
    fn extract_to_xhtml(&self, pdf_bytes: &[u8]) -> Result<String> {
        println!("🔧 Processing {} bytes through JNI", pdf_bytes.len());

        // Attach current thread to JVM
        // This is safe to call multiple times - returns existing env if already attached
        let mut env = self
            .jvm
            .attach_current_thread()
            .map_err(|e| anyhow!("Failed to attach thread to JVM: {:?}", e))?;

        // Convert Rust bytes to Java byte array
        let java_bytes = env
            .byte_array_from_slice(pdf_bytes)
            .map_err(|e| anyhow!("Failed to create Java byte array: {:?}", e))?;

        // Call static method: TikaMain.processToXhtml(byte[]) -> String
        let result = env.call_static_method(
            "com/blazegraph/TikaMain",
            "processToXhtml",
            "([B)Ljava/lang/String;",
            &[(&java_bytes).into()],
        );

        // Handle Java exceptions
        if env
            .exception_check()
            .map_err(|e| anyhow!("Failed to check for exception: {:?}", e))?
        {
            env.exception_describe()
                .map_err(|e| anyhow!("Failed to describe exception: {:?}", e))?;
            env.exception_clear()
                .map_err(|e| anyhow!("Failed to clear exception: {:?}", e))?;
            return Err(anyhow!("Java exception during PDF processing"));
        }

        let result = result.map_err(|e| anyhow!("JNI call failed: {:?}", e))?;

        // Extract string from result
        let jstring = result
            .l()
            .map_err(|e| anyhow!("Expected String result: {:?}", e))?;

        let output: String = env
            .get_string((&jstring).into())
            .map_err(|e| anyhow!("Failed to convert Java string: {:?}", e))?
            .into();

        println!(
            "✅ JNI processing completed, output size: {} characters",
            output.len()
        );
        Ok(output)
    }

    fn name(&self) -> &str {
        "TikaJniBackend"
    }

    fn is_healthy(&self) -> bool {
        // Try to attach thread as a health check
        self.jvm.attach_current_thread().is_ok()
    }
}

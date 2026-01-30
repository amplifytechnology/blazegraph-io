//! JRE Manager - Auto-download and manage Java Runtime for JNI backend
//!
//! Downloads Eclipse Temurin (Adoptium) JRE on first use if not present.
//! Stores JRE in user's data directory for reuse across invocations.

use anyhow::{anyhow, Context, Result};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};

/// JRE version to download (LTS version for stability)
const JRE_VERSION: &str = "21";

/// Manages JRE installation for the CLI
pub struct JreManager {
    /// Base directory for blazegraph data (e.g., ~/.local/share/blazegraph)
    data_dir: PathBuf,
}

impl JreManager {
    /// Create a new JreManager using the default data directory
    pub fn new() -> Result<Self> {
        let data_dir = Self::get_data_dir()?;
        Ok(Self { data_dir })
    }

    /// Get the data directory (~/.local/share/blazegraph on all Unix platforms)
    fn get_data_dir() -> Result<PathBuf> {
        // Use ~/.local/share/blazegraph consistently on macOS/Linux
        // This is more predictable than platform-specific paths like ~/Library/Application Support
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow!("Could not determine home directory"))?;

        #[cfg(windows)]
        {
            // On Windows, use the standard local app data location
            let base = dirs::data_local_dir()
                .ok_or_else(|| anyhow!("Could not determine local data directory"))?;
            Ok(base.join("blazegraph"))
        }

        #[cfg(not(windows))]
        {
            // On macOS/Linux, use ~/.local/share/blazegraph
            Ok(home.join(".local").join("share").join("blazegraph"))
        }
    }

    /// Get the path where JRE should be installed
    pub fn jre_path(&self) -> PathBuf {
        self.data_dir.join("jre")
    }

    /// Get the path to the bundled JAR file
    /// Returns the path relative to the executable or the development path
    pub fn find_jar_path() -> Result<PathBuf> {
        // Check various locations for the JAR
        let candidates = [
            // Core deps path (running from blazegraph-cli directory)
            PathBuf::from("../blazegraph-core/deps/tika/jni-jars/blazing-tika-jni.jar"),
            // Core deps path (running from workspace root, e.g. blazegraph-io/)
            PathBuf::from("blazegraph-core/deps/tika/jni-jars/blazing-tika-jni.jar"),
            // Core deps path (running from parent of workspace)
            PathBuf::from("blazegraph-io/blazegraph-core/deps/tika/jni-jars/blazing-tika-jni.jar"),
            // Legacy CLI path (for backwards compat)
            PathBuf::from("src/tika/jars/blazing-tika-jni.jar"),
            // Installed alongside binary
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("blazing-tika-jni.jar")))
                .unwrap_or_default(),
            // In data directory (for future auto-download)
            Self::get_data_dir()
                .ok()
                .map(|p| p.join("blazing-tika-jni.jar"))
                .unwrap_or_default(),
        ];

        for candidate in &candidates {
            if candidate.exists() && candidate.to_string_lossy().len() > 0 {
                return Ok(candidate.clone());
            }
        }

        Err(anyhow!(
            "Could not find blazing-tika JAR file.\n\
             Searched in:\n\
             - ../blazegraph-core/deps/tika/jni-jars/blazing-tika-jni.jar\n\
             - blazegraph-core/deps/tika/jni-jars/blazing-tika-jni.jar\n\
             - blazegraph-io/blazegraph-core/deps/tika/jni-jars/blazing-tika-jni.jar\n\
             - src/tika/jars/blazing-tika-jni.jar (legacy)\n\
             - Next to executable\n\
             - Data directory"
        ))
    }

    /// Check if JRE is already installed
    pub fn is_jre_installed(&self) -> bool {
        let jre_path = self.jre_path();
        // Check for the java binary as proof of installation
        let java_binary = if cfg!(windows) {
            jre_path.join("bin").join("java.exe")
        } else {
            jre_path.join("bin").join("java")
        };
        java_binary.exists()
    }

    /// Ensure JRE is available, downloading if necessary
    /// Returns the path to the JRE directory
    pub fn ensure_jre(&self) -> Result<PathBuf> {
        let jre_path = self.jre_path();

        if self.is_jre_installed() {
            println!("✅ JRE found at: {}", jre_path.display());
            return Ok(jre_path);
        }

        println!(
            "📦 JRE not found, downloading Eclipse Temurin {}...",
            JRE_VERSION
        );
        self.download_and_install_jre()?;

        Ok(jre_path)
    }

    /// Download and install JRE from Adoptium
    fn download_and_install_jre(&self) -> Result<()> {
        // Ensure data directory exists
        fs::create_dir_all(&self.data_dir).with_context(|| {
            format!(
                "Failed to create data directory: {}",
                self.data_dir.display()
            )
        })?;

        // Detect platform
        let platform = Platform::detect()?;
        println!("   Platform: {}-{}", platform.os, platform.arch);

        // Build download URL
        let url = platform.adoptium_url(JRE_VERSION);
        println!("   URL: {}", url);

        // Download to temp file
        let temp_path = self.data_dir.join("jre_download.tmp");
        self.download_file(&url, &temp_path)?;

        // Extract archive
        println!("📂 Extracting JRE...");
        let jre_path = self.jre_path();

        // Remove existing JRE directory if it exists (partial install)
        if jre_path.exists() {
            fs::remove_dir_all(&jre_path)
                .with_context(|| "Failed to remove existing JRE directory")?;
        }

        self.extract_archive(&temp_path, &platform)?;

        // Cleanup temp file
        let _ = fs::remove_file(&temp_path);

        // Verify installation
        if self.is_jre_installed() {
            println!("✅ JRE installed successfully at: {}", jre_path.display());
            Ok(())
        } else {
            Err(anyhow!(
                "JRE installation failed - java binary not found after extraction"
            ))
        }
    }

    /// Download a file with progress indication
    fn download_file(&self, url: &str, dest: &Path) -> Result<()> {
        let response = ureq::get(url)
            .call()
            .with_context(|| format!("Failed to download from {}", url))?;

        let total_size = response
            .header("Content-Length")
            .and_then(|s| s.parse::<u64>().ok());

        let mut reader = response.into_reader();
        let mut file = File::create(dest)
            .with_context(|| format!("Failed to create file: {}", dest.display()))?;

        let mut downloaded: u64 = 0;
        let mut buffer = [0u8; 8192];
        let mut last_progress = 0;

        loop {
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }

            file.write_all(&buffer[..bytes_read])?;
            downloaded += bytes_read as u64;

            // Print progress every 10%
            if let Some(total) = total_size {
                let progress = ((downloaded * 100) / total) as usize;
                if progress >= last_progress + 10 {
                    print!(
                        "\r   Downloading: {}% ({:.1} MB)",
                        progress,
                        downloaded as f64 / 1_000_000.0
                    );
                    io::stdout().flush()?;
                    last_progress = progress;
                }
            }
        }

        if total_size.is_some() {
            println!("\r   Downloading: 100%                    ");
        }

        Ok(())
    }

    /// Extract the downloaded archive
    fn extract_archive(&self, archive_path: &Path, platform: &Platform) -> Result<()> {
        let jre_path = self.jre_path();

        if platform.is_zip() {
            self.extract_zip(archive_path, &jre_path)?;
        } else {
            self.extract_tar_gz(archive_path, &jre_path)?;
        }

        Ok(())
    }

    /// Extract a .tar.gz archive (Linux/macOS)
    fn extract_tar_gz(&self, archive_path: &Path, dest: &Path) -> Result<()> {
        let file = File::open(archive_path)
            .with_context(|| format!("Failed to open archive: {}", archive_path.display()))?;

        let decoder = flate2::read::GzDecoder::new(BufReader::new(file));
        let mut archive = tar::Archive::new(decoder);

        // Create a temp extraction directory
        let temp_extract = self.data_dir.join("jre_extract_tmp");
        if temp_extract.exists() {
            fs::remove_dir_all(&temp_extract)?;
        }
        fs::create_dir_all(&temp_extract)?;

        archive
            .unpack(&temp_extract)
            .with_context(|| "Failed to extract tar.gz archive")?;

        // Adoptium archives have a top-level directory like "jdk-21.0.2+13-jre"
        // We need to move its contents to our jre_path
        self.flatten_extracted_dir(&temp_extract, dest)?;

        // Cleanup
        let _ = fs::remove_dir_all(&temp_extract);

        Ok(())
    }

    /// Extract a .zip archive (Windows)
    fn extract_zip(&self, archive_path: &Path, dest: &Path) -> Result<()> {
        let file = File::open(archive_path)
            .with_context(|| format!("Failed to open archive: {}", archive_path.display()))?;

        let mut archive = zip::ZipArchive::new(BufReader::new(file))
            .with_context(|| "Failed to read zip archive")?;

        // Create a temp extraction directory
        let temp_extract = self.data_dir.join("jre_extract_tmp");
        if temp_extract.exists() {
            fs::remove_dir_all(&temp_extract)?;
        }
        fs::create_dir_all(&temp_extract)?;

        archive
            .extract(&temp_extract)
            .with_context(|| "Failed to extract zip archive")?;

        // Flatten the extracted directory structure
        self.flatten_extracted_dir(&temp_extract, dest)?;

        // Cleanup
        let _ = fs::remove_dir_all(&temp_extract);

        Ok(())
    }

    /// Move contents from extracted subdirectory to final destination
    /// Adoptium archives contain a top-level dir like "jdk-21.0.2+13-jre/"
    fn flatten_extracted_dir(&self, extracted: &Path, dest: &Path) -> Result<()> {
        // Find the single subdirectory in the extracted location
        let entries: Vec<_> = fs::read_dir(extracted)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        let source = if entries.len() == 1 {
            entries[0].path()
        } else {
            // No subdirectory or multiple - use extracted directly
            extracted.to_path_buf()
        };

        // On macOS, the structure is different - JRE is inside Contents/Home
        let actual_source = if cfg!(target_os = "macos") {
            let contents_home = source.join("Contents").join("Home");
            if contents_home.exists() {
                contents_home
            } else {
                source
            }
        } else {
            source
        };

        // Move to final destination
        fs::rename(&actual_source, dest)
            .or_else(|_| {
                // rename might fail across filesystems, fall back to copy
                Self::copy_dir_recursive(&actual_source, dest)
            })
            .with_context(|| {
                format!(
                    "Failed to move JRE from {} to {}",
                    actual_source.display(),
                    dest.display()
                )
            })?;

        Ok(())
    }

    /// Recursively copy a directory
    fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
        fs::create_dir_all(dst)?;

        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if src_path.is_dir() {
                Self::copy_dir_recursive(&src_path, &dst_path)?;
            } else {
                fs::copy(&src_path, &dst_path)?;
            }
        }

        Ok(())
    }
}

/// Platform detection for download URL construction
struct Platform {
    os: &'static str,
    arch: &'static str,
}

impl Platform {
    /// Detect the current platform
    fn detect() -> Result<Self> {
        let os = if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "macos") {
            "mac"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            return Err(anyhow!("Unsupported operating system"));
        };

        let arch = if cfg!(target_arch = "x86_64") {
            "x64"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            return Err(anyhow!("Unsupported architecture"));
        };

        Ok(Self { os, arch })
    }

    /// Build the Adoptium download URL
    fn adoptium_url(&self, version: &str) -> String {
        // Adoptium API v3 binary endpoint
        // https://api.adoptium.net/v3/binary/latest/{feature_version}/{release_type}/{os}/{arch}/{image_type}/{jvm_impl}/{heap_size}/{vendor}
        format!(
            "https://api.adoptium.net/v3/binary/latest/{}/ga/{}/{}/jre/hotspot/normal/eclipse",
            version, self.os, self.arch
        )
    }

    /// Check if this platform uses zip (Windows) or tar.gz (Linux/macOS)
    fn is_zip(&self) -> bool {
        self.os == "windows"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detection() {
        let platform = Platform::detect().unwrap();
        // Just verify it doesn't panic
        assert!(!platform.os.is_empty());
        assert!(!platform.arch.is_empty());
    }

    #[test]
    fn test_adoptium_url_format() {
        let platform = Platform {
            os: "linux",
            arch: "x64",
        };
        let url = platform.adoptium_url("21");
        assert!(url.contains("adoptium.net"));
        assert!(url.contains("linux"));
        assert!(url.contains("x64"));
        assert!(url.contains("jre"));
    }
}

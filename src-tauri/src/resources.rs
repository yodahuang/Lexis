//! Resource management system for Lexis
//!
//! Handles auto-downloading and caching of NLP models and dictionaries.
//! All resources are stored in the XDG data directory.

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

/// Base URL for HuggingFace model downloads
const HUGGINGFACE_BASE: &str = "https://huggingface.co";

/// GLiNER model repository on HuggingFace
const GLINER_REPO: &str = "onnx-community/gliner_large-v2.1";

/// SymSpell dictionary URL
const SYMSPELL_DICT_URL: &str = "https://raw.githubusercontent.com/wolfgarbe/SymSpell/master/SymSpell/frequency_dictionary_en_82_765.txt";

/// Progress callback for resource downloads
pub type ProgressCallback = Box<dyn Fn(&str, u64, u64) + Send>;

/// Get the base resource directory (XDG data directory)
pub fn get_resource_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lexis")
        .join("resources")
}

/// Get the GLiNER model directory
pub fn get_gliner_dir() -> PathBuf {
    get_resource_dir().join("gliner")
}

/// Get the SymSpell dictionary directory
pub fn get_symspell_dir() -> PathBuf {
    get_resource_dir().join("symspell")
}

/// Check if GLiNER model is available
pub fn is_gliner_available() -> bool {
    let dir = get_gliner_dir();
    dir.join("model.onnx").exists() && dir.join("tokenizer.json").exists()
}

/// Check if SymSpell dictionary is available
pub fn is_symspell_available() -> bool {
    get_symspell_dir().join("frequency_dictionary_en_82_765.txt").exists()
}

/// Resource download status
#[derive(Debug, Clone)]
pub enum DownloadStatus {
    AlreadyExists,
    Downloading { file: String, progress: u64, total: u64 },
    Completed,
    Failed(String),
}

/// Ensure GLiNER model is available, downloading if necessary
/// Returns the model directory path
pub fn ensure_gliner_model<F>(on_progress: F) -> Result<PathBuf, String>
where
    F: Fn(DownloadStatus) + Send,
{
    let model_dir = get_gliner_dir();
    let model_path = model_dir.join("model.onnx");
    let tokenizer_path = model_dir.join("tokenizer.json");

    if model_path.exists() && tokenizer_path.exists() {
        on_progress(DownloadStatus::AlreadyExists);
        return Ok(model_dir);
    }

    // Create directory
    fs::create_dir_all(&model_dir)
        .map_err(|e| format!("Failed to create model directory: {}", e))?;

    // Download tokenizer.json first (smaller file)
    if !tokenizer_path.exists() {
        let url = format!("{}/{}/resolve/main/tokenizer.json", HUGGINGFACE_BASE, GLINER_REPO);
        eprintln!("Downloading GLiNER tokenizer from {}...", url);
        download_file(&url, &tokenizer_path, |progress, total| {
            on_progress(DownloadStatus::Downloading {
                file: "tokenizer.json".to_string(),
                progress,
                total,
            });
        })?;
    }

    // Download model.onnx (large file ~650MB)
    if !model_path.exists() {
        let url = format!("{}/{}/resolve/main/onnx/model.onnx", HUGGINGFACE_BASE, GLINER_REPO);
        eprintln!("Downloading GLiNER model from {}...", url);
        eprintln!("This is a large file (~650MB), please wait...");
        download_file(&url, &model_path, |progress, total| {
            on_progress(DownloadStatus::Downloading {
                file: "model.onnx".to_string(),
                progress,
                total,
            });
        })?;
    }

    on_progress(DownloadStatus::Completed);
    eprintln!("GLiNER model downloaded successfully to {:?}", model_dir);
    Ok(model_dir)
}

/// Ensure SymSpell dictionary is available, downloading if necessary
/// Returns the dictionary file path
pub fn ensure_symspell_dict<F>(on_progress: F) -> Result<PathBuf, String>
where
    F: Fn(DownloadStatus) + Send,
{
    let dict_dir = get_symspell_dir();
    let dict_path = dict_dir.join("frequency_dictionary_en_82_765.txt");

    if dict_path.exists() {
        on_progress(DownloadStatus::AlreadyExists);
        return Ok(dict_path);
    }

    // Create directory
    fs::create_dir_all(&dict_dir)
        .map_err(|e| format!("Failed to create dictionary directory: {}", e))?;

    eprintln!("Downloading SymSpell dictionary...");
    download_file(SYMSPELL_DICT_URL, &dict_path, |progress, total| {
        on_progress(DownloadStatus::Downloading {
            file: "frequency_dictionary_en_82_765.txt".to_string(),
            progress,
            total,
        });
    })?;

    on_progress(DownloadStatus::Completed);
    eprintln!("SymSpell dictionary downloaded successfully to {:?}", dict_path);
    Ok(dict_path)
}

/// Download a file with progress tracking
fn download_file<F>(url: &str, dest: &PathBuf, on_progress: F) -> Result<(), String>
where
    F: Fn(u64, u64),
{
    let response = ureq::get(url)
        .call()
        .map_err(|e| format!("Failed to download {}: {}", url, e))?;

    let total_size = response
        .header("content-length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let mut reader = response.into_reader();

    // Use a temporary file to avoid partial downloads
    let temp_path = dest.with_extension("download");
    let mut file = fs::File::create(&temp_path)
        .map_err(|e| format!("Failed to create file: {}", e))?;

    let mut downloaded: u64 = 0;
    let mut buffer = [0u8; 8192];
    let mut last_progress_update = std::time::Instant::now();

    loop {
        let bytes_read = reader.read(&mut buffer)
            .map_err(|e| format!("Failed to read response: {}", e))?;

        if bytes_read == 0 {
            break;
        }

        file.write_all(&buffer[..bytes_read])
            .map_err(|e| format!("Failed to write file: {}", e))?;

        downloaded += bytes_read as u64;

        // Update progress at most every 100ms to avoid flooding
        if last_progress_update.elapsed().as_millis() >= 100 {
            on_progress(downloaded, total_size);
            last_progress_update = std::time::Instant::now();
        }
    }

    // Final progress update
    on_progress(downloaded, total_size);

    // Rename temp file to final destination
    fs::rename(&temp_path, dest)
        .map_err(|e| format!("Failed to finalize download: {}", e))?;

    Ok(())
}

/// Get status of all resources
pub fn get_resource_status() -> ResourceStatus {
    ResourceStatus {
        gliner_available: is_gliner_available(),
        gliner_path: get_gliner_dir(),
        symspell_available: is_symspell_available(),
        symspell_path: get_symspell_dir().join("frequency_dictionary_en_82_765.txt"),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ResourceStatus {
    pub gliner_available: bool,
    pub gliner_path: PathBuf,
    pub symspell_available: bool,
    pub symspell_path: PathBuf,
}

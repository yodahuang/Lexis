mod calibre;
mod epub;
pub mod nlp;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Emitter;

pub struct AppState {
    pub library_path: Mutex<Option<String>>,
    pub nlp: nlp::NlpPipeline,
    /// Active analysis jobs: book_id -> cancellation token
    pub active_jobs: Mutex<HashMap<i64, Arc<AtomicBool>>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            library_path: Mutex::new(None),
            nlp: nlp::NlpPipeline::new(),
            active_jobs: Mutex::new(HashMap::new()),
        }
    }
}

#[tauri::command]
fn scan_library(path: &str, state: tauri::State<AppState>) -> Result<Vec<calibre::Book>, calibre::CalibreError> {
    let books = calibre::scan_library(path)?;
    *state.library_path.lock().unwrap() = Some(path.to_string());
    Ok(books)
}

#[tauri::command]
fn get_epub_path(book_id: i64, state: tauri::State<AppState>) -> Result<Option<String>, String> {
    let lib_path = state.library_path.lock().unwrap();
    let lib_path = lib_path.as_ref().ok_or("No library loaded")?;

    calibre::get_epub_path(lib_path, book_id)
        .map(|p| p.map(|path| path.to_string_lossy().to_string()))
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct BookText {
    text: String,
    chapter_count: usize,
    word_count: usize,
}

#[tauri::command]
fn get_book_text(book_id: i64, state: tauri::State<AppState>) -> Result<BookText, String> {
    let lib_path = state.library_path.lock().unwrap();
    let lib_path = lib_path.as_ref().ok_or("No library loaded")?;

    let epub_path = calibre::get_epub_path(lib_path, book_id)
        .map_err(|e| e.to_string())?
        .ok_or("No EPUB file found for this book")?;

    let extracted = epub::extract_text(&epub_path).map_err(|e| e.to_string())?;

    let word_count = extracted.full_text.split_whitespace().count();

    Ok(BookText {
        text: extracted.full_text,
        chapter_count: extracted.chapter_count,
        word_count,
    })
}

#[derive(serde::Serialize)]
struct AnalysisResult {
    book_id: i64,
    word_count: usize,
    hard_words: Vec<nlp::HardWord>,
    stats: nlp::AnalysisStats,
}

#[derive(serde::Serialize, Clone)]
struct AnalysisProgress {
    book_id: i64,
    stage: String,
    progress: u8, // 0-100
    detail: Option<String>,
    sample_words: Option<Vec<nlp::SampleWord>>,
}

#[tauri::command]
async fn analyze_book(
    book_id: i64,
    frequency_threshold: Option<f32>,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<AnalysisResult, String> {
    let threshold = frequency_threshold.unwrap_or(0.00005);

    // Create cancellation token and register the job
    let cancel_token = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = state.active_jobs.lock().unwrap();
        // Cancel any existing job for this book
        if let Some(old_token) = jobs.get(&book_id) {
            old_token.store(true, Ordering::SeqCst);
        }
        jobs.insert(book_id, Arc::clone(&cancel_token));
    }

    let lib_path = {
        let guard = state.library_path.lock().unwrap();
        guard.clone().ok_or("No library loaded")?
    };

    let epub_path = calibre::get_epub_path(&lib_path, book_id)
        .map_err(|e| e.to_string())?
        .ok_or("No EPUB file found for this book")?;

    // Check cancellation before expensive operation
    if cancel_token.load(Ordering::SeqCst) {
        cleanup_job(&state, book_id);
        return Err("Analysis cancelled".to_string());
    }

    let _ = window.emit("analysis-progress", AnalysisProgress {
        book_id,
        stage: "Extracting text".to_string(),
        progress: 10,
        detail: Some("Reading EPUB...".to_string()),
        sample_words: None,
    });

    let extracted = epub::extract_text(&epub_path).map_err(|e| e.to_string())?;
    let word_count = extracted.full_text.split_whitespace().count();

    // Check cancellation before NLP
    if cancel_token.load(Ordering::SeqCst) {
        cleanup_job(&state, book_id);
        return Err("Analysis cancelled".to_string());
    }

    // Run NLP analysis with progress callback and cancellation check
    let nlp = &state.nlp;
    let window_clone = window.clone();
    let cancel_clone = Arc::clone(&cancel_token);
    let result = nlp.analyze_with_cancel(&extracted.full_text, threshold, &cancel_clone, |progress| {
        let _ = window_clone.emit("analysis-progress", AnalysisProgress {
            book_id,
            stage: progress.stage,
            progress: progress.progress,
            detail: progress.detail,
            sample_words: progress.sample_words,
        });
    });

    // Clean up job tracking
    cleanup_job(&state, book_id);

    let (hard_words, stats) = result.ok_or("Analysis cancelled")?;

    let _ = window.emit("analysis-progress", AnalysisProgress {
        book_id,
        stage: "Analysis complete!".to_string(),
        progress: 100,
        detail: Some(format!("{} words found, {} filtered", hard_words.len(), stats.filtered_by_ner.len())),
        sample_words: None,
    });

    Ok(AnalysisResult {
        book_id,
        word_count,
        hard_words,
        stats,
    })
}

fn cleanup_job(state: &tauri::State<'_, AppState>, book_id: i64) {
    let mut jobs = state.active_jobs.lock().unwrap();
    jobs.remove(&book_id);
}

#[tauri::command]
fn cancel_analysis(book_id: i64, state: tauri::State<'_, AppState>) -> bool {
    let jobs = state.active_jobs.lock().unwrap();
    if let Some(token) = jobs.get(&book_id) {
        token.store(true, Ordering::SeqCst);
        eprintln!("Cancelling analysis for book {}", book_id);
        true
    } else {
        false
    }
}

#[tauri::command]
fn get_active_jobs(state: tauri::State<'_, AppState>) -> Vec<i64> {
    let jobs = state.active_jobs.lock().unwrap();
    jobs.keys().cloned().collect()
}

#[tauri::command]
fn export_json(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![scan_library, get_epub_path, get_book_text, analyze_book, export_json, cancel_analysis, get_active_jobs])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

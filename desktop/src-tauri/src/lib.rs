mod calibre;
mod epub;
mod nlp;

use std::sync::Mutex;
use tauri::Emitter;

pub struct AppState {
    pub library_path: Mutex<Option<String>>,
    pub nlp: nlp::NlpPipeline,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            library_path: Mutex::new(None),
            nlp: nlp::NlpPipeline::new(),
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
}

#[tauri::command]
async fn analyze_book(
    book_id: i64,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<AnalysisResult, String> {
    let lib_path = {
        let guard = state.library_path.lock().unwrap();
        guard.clone().ok_or("No library loaded")?
    };

    let epub_path = calibre::get_epub_path(&lib_path, book_id)
        .map_err(|e| e.to_string())?
        .ok_or("No EPUB file found for this book")?;

    // Emit progress: extracting text
    let _ = window.emit("analysis-progress", AnalysisProgress {
        book_id,
        stage: "Extracting text".to_string(),
        progress: 10,
        detail: Some("Reading EPUB...".to_string()),
    });

    let extracted = epub::extract_text(&epub_path).map_err(|e| e.to_string())?;
    let word_count = extracted.full_text.split_whitespace().count();

    // Run NLP analysis with progress callback
    let nlp = &state.nlp;
    let window_clone = window.clone();
    let (hard_words, stats) = nlp.analyze(&extracted.full_text, |progress| {
        let _ = window_clone.emit("analysis-progress", AnalysisProgress {
            book_id,
            stage: progress.stage,
            progress: progress.progress,
            detail: progress.detail,
        });
    });

    // Emit progress: complete
    let _ = window.emit("analysis-progress", AnalysisProgress {
        book_id,
        stage: "Analysis complete!".to_string(),
        progress: 100,
        detail: Some(format!("{} words found, {} filtered", hard_words.len(), stats.filtered_by_ner.len())),
    });

    Ok(AnalysisResult {
        book_id,
        word_count,
        hard_words,
        stats,
    })
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
        .invoke_handler(tauri::generate_handler![scan_library, get_epub_path, get_book_text, analyze_book, export_json])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct Book {
    pub id: i64,
    pub title: String,
    pub author: String,
    pub path: String,
    pub cover_path: Option<String>,
    pub has_epub: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CalibreError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Library not found at path: {0}")]
    LibraryNotFound(String),
    #[error("Invalid library path: {0}")]
    InvalidPath(String),
}

impl Serialize for CalibreError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub fn scan_library(library_path: &str) -> Result<Vec<Book>, CalibreError> {
    let lib_path = Path::new(library_path);
    let db_path = lib_path.join("metadata.db");

    if !db_path.exists() {
        return Err(CalibreError::LibraryNotFound(library_path.to_string()));
    }

    let db_uri = format!(
        "file:{}?mode=ro",
        db_path.to_str().ok_or_else(|| CalibreError::InvalidPath(library_path.to_string()))?
    );

    let conn = Connection::open_with_flags(
        &db_uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;

    let mut stmt = conn.prepare(
        r#"
        SELECT
            b.id,
            b.title,
            b.path,
            COALESCE(GROUP_CONCAT(a.name, ' & '), 'Unknown') as author,
            b.has_cover
        FROM books b
        LEFT JOIN books_authors_link bal ON b.id = bal.book
        LEFT JOIN authors a ON bal.author = a.id
        GROUP BY b.id
        ORDER BY b.title
        "#,
    )?;

    let books = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let title: String = row.get(1)?;
            let book_path: String = row.get(2)?;
            let author: String = row.get(3)?;
            let has_cover: bool = row.get(4)?;

            let full_book_path = lib_path.join(&book_path);
            let cover_path = if has_cover {
                let cover = full_book_path.join("cover.jpg");
                if cover.exists() {
                    Some(cover.to_string_lossy().to_string())
                } else {
                    None
                }
            } else {
                None
            };

            // Check if EPUB exists
            let has_epub = find_epub(&full_book_path).is_some();

            Ok(Book {
                id,
                title,
                author,
                path: full_book_path.to_string_lossy().to_string(),
                cover_path,
                has_epub,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(books)
}

pub fn find_epub(book_dir: &Path) -> Option<PathBuf> {
    if let Ok(entries) = std::fs::read_dir(book_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "epub").unwrap_or(false) {
                return Some(path);
            }
        }
    }
    None
}

pub fn get_epub_path(library_path: &str, book_id: i64) -> Result<Option<PathBuf>, CalibreError> {
    let lib_path = Path::new(library_path);
    let db_path = lib_path.join("metadata.db");

    let db_uri = format!(
        "file:{}?mode=ro",
        db_path.to_str().ok_or_else(|| CalibreError::InvalidPath(library_path.to_string()))?
    );

    let conn = Connection::open_with_flags(
        &db_uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;

    let book_path: String = conn.query_row(
        "SELECT path FROM books WHERE id = ?",
        [book_id],
        |row| row.get(0),
    )?;

    let full_path = lib_path.join(&book_path);
    Ok(find_epub(&full_path))
}

# Lexis - Desktop App

Desktop companion app for vocabulary extraction from your Calibre ebook library. Analyzes books to find "hard words" and exports structured data for the iOS companion app.

## Two-App Architecture

- **Desktop (this repo)**: Calibre integration, NLP analysis, structured export
- **iOS (separate repo)**: Vocabulary practice, spaced repetition, mobile-first UX

The desktop app is the "brain" - it does heavy NLP processing on your ebook library and exports vocab data. The iOS app is the "trainer" - optimized for on-the-go learning.

## Stack

- **Framework**: Tauri 2.x (Rust backend + Svelte frontend)
- **Data Source**: Calibre library (SQLite, read-only)
- **NLP**: wordfreq, unicode-segmentation, gliner (NER)
- **Export**: JSON (for iOS app consumption)

## Data Flow

1. **Discovery**: Query Calibre's `metadata.db` for EPUB books + covers
2. **Ingestion**: Load `.epub` → extract HTML → sanitize to text
3. **Analysis** (background thread):
   - Tokenize into sentences
   - Filter proper nouns (capitalization heuristic + NER)
   - Score words by frequency
   - Return "hard words" + context sentences
4. **Export**: JSON file with vocab data for iOS app

## Export Format

```json
{
  "version": 1,
  "exported_at": "2024-01-15T10:30:00Z",
  "books": [
    {
      "id": "calibre-123",
      "title": "Book Title",
      "author": "Author Name",
      "words": [
        {
          "word": "ephemeral",
          "frequency_score": 0.0001,
          "contexts": [
            "The ephemeral beauty of cherry blossoms..."
          ]
        }
      ]
    }
  ]
}
```

## Development

```bash
devenv shell           # Enter dev environment
cd desktop && bun install
cargo tauri dev        # Run in dev mode
```

## Project Structure

```
lexis/
├── desktop/              # Tauri app
│   ├── src/              # Svelte frontend
│   ├── src-tauri/        # Rust backend
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── calibre.rs    # Calibre DB queries
│   │   │   ├── epub.rs       # EPUB text extraction
│   │   │   ├── nlp.rs        # Word frequency analysis
│   │   │   └── export.rs     # JSON export
│   │   └── Cargo.toml
│   └── package.json
├── devenv.nix
└── CLAUDE.md
```

## Implementation Phases

### Phase 1: Tauri + Calibre Bridge

- Scaffold Tauri 2.x + Svelte project
- `scan_library(path)` command → returns book metadata
- Read-only SQLite connection (handle Calibre lock)

### Phase 2: EPUB Parsing

- `get_book_text(book_id)` command
- Extract + sanitize HTML from chapters
- Crates: `epub`, `ammonia`

### Phase 3: NLP Pipeline

- `analyze_book(book_id)` command (runs in background thread)
- Sentence tokenization, word frequency scoring
- Filter entities (names, places) via NER
- Crates: `wordfreq`, `unicode-segmentation`, `gliner`

### Phase 4: UI + Export

- Book grid with covers
- Analysis progress indicator
- Export to JSON for iOS app

## Critical Pitfalls

### Calibre Database Lock

Calibre locks its DB. Always open read-only:

```rust
let conn = Connection::open_with_flags(
    format!("file:{}?mode=ro", db_path),
    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI
)?;
```

### Blocking UI

NLP is slow. Never run in main thread:

```rust
tokio::task::spawn_blocking(|| {
    // heavy NLP work here
})
```

### Asset Paths in WebView

Use Tauri's asset protocol for local files:

```typescript
import { convertFileSrc } from '@tauri-apps/api/core';
```

## Code Standards

- No `.unwrap()` - propagate errors to frontend
- Load NLP models once at startup via `tauri::State`
- TypeScript strict mode in frontend

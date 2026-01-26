# Lexis - Desktop App

Desktop companion app for vocabulary extraction from your Calibre ebook library. Analyzes books to find "hard words" and exports structured data for the iOS companion app.

## Two-App Architecture

- **Desktop (this repo)**: Calibre integration, NLP analysis, structured export
- **iOS (separate repo)**: Vocabulary practice, spaced repetition, mobile-first UX

The desktop app is the "brain" - it does heavy NLP processing on your ebook library and exports vocab data. The iOS app is the "trainer" - optimized for on-the-go learning.

## Stack

- **Framework**: Tauri 2.x (Rust backend + Svelte 5 frontend with runes)
- **Data Source**: Calibre library (SQLite, read-only)
- **NLP**: wordfreq, rust-stemmers, symspell, gliner (NER)
- **UI**: Claymorphism design with Motion One animations
- **Export**: JSON (for iOS app consumption)

## Data Flow

1. **Discovery**: Query Calibre's `metadata.db` for EPUB books + covers
2. **Ingestion**: Load `.epub` → extract HTML → sanitize to text
3. **Analysis** (runs async, emits progress events):
   - Tokenize into sentences (unicode-segmentation)
   - **Wordfreq filtering** (FIRST): Filter by frequency threshold (configurable, default 0.00005)
   - **Malformed word filter**: symspell segmentation for EPUB errors (only on words NOT in dictionary)
   - **Porter stemming**: Normalize word forms (running → run, gaieties → gaiety)
   - **NER filtering** (LAST, only on candidates): GLiNER to remove proper nouns (names, places)
   - Return "hard words" + ALL context sentences
4. **Export**: JSON file with vocab data for iOS app

## NLP Pipeline Details (nlp.rs)

### Current Architecture

```
Text → Sentences → Words → Wordfreq Filter → Malformed Filter → Stemming → NER Filter → Results
                           (freq < threshold)  (symspell)        (rust-stemmers) (GLiNER)
```

### Key Design Decisions

1. **Wordfreq FIRST**: Fast filtering reduces candidates before expensive NER
2. **Symspell only for unknown words**: Words in dictionary (freq > 0) are valid, never filter them
3. **GLiNER lazy-loaded**: Model downloaded on first use (~650MB), uses CoreML on macOS
4. **Progress callbacks**: Real-time UI updates with sample words being classified

### Models (auto-downloaded to resources/)

- `gliner/model.onnx` + `tokenizer.json` (~650MB) - NER model
- `symspell/frequency_dictionary_en_82_765.txt` (~1.4MB) - Word segmentation dictionary

### Known Issues / TODO

- **Job cancellation**: Closing modal doesn't cancel background analysis
- **Job queue**: Multiple analyses can run simultaneously (interleaved progress)
- **Normalization timing**: Words like "blaster's" shown in progress before normalization

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
- Load NLP models once at startup via `tauri::State` and `OnceLock`
- TypeScript strict mode in frontend
- Svelte 5 runes: `$state`, `$effect`, `$derived`

## Testing the NLP Pipeline

To debug word filtering, analyze a book and check console output:

```bash
cargo tauri dev
# Select a book, run analysis, watch stderr for:
# - "Filtering malformed word 'X' -> 'Y'" (symspell)
# - "Found N hard word candidates after wordfreq filtering"
# - "Running NER on N sentences"
# - "GLiNER found N unique entities"
```

### Common False Positives to Watch For

If symspell filters valid words like "favorites", "neighboring", "traveled":
- These ARE dictionary words (freq > 0) → should NOT be filtered
- Fix: Check `wordfreq.word_frequency(word) > 0.0` BEFORE running symspell

### Test Cases for Malformed Word Detection

Should FILTER (concatenated EPUB errors):
- "believethat's" → "believe that" ✓
- "theendofeternity" → "the end of eternity" ✓
- "meetshimself" → "meets himself" ✓

Should KEEP (valid dictionary words):
- "favorites" (freq > 0) ✓
- "neighboring" (freq > 0) ✓
- "traveled" (freq > 0) ✓
- "indifferent" (freq > 0) ✓

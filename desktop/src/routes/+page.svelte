<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open, save } from "@tauri-apps/plugin-dialog";
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount, onDestroy } from "svelte";
  import { animate, stagger } from "motion";

  interface Book {
    id: number;
    title: string;
    author: string;
    path: string;
    cover_path: string | null;
    has_epub: boolean;
  }

  interface HardWord {
    word: string;
    frequency_score: number;
    contexts: string[];
    count: number;
    variants: string[];
  }

  interface AnalysisStats {
    total_candidates: number;
    filtered_by_ner: string[];
    hard_words_count: number;
  }

  interface AnalysisResult {
    book_id: number;
    word_count: number;
    hard_words: HardWord[];
    stats: AnalysisStats;
  }

  // Highlight word in context
  function highlightWord(context: string, word: string, variants: string[]): string {
    const allForms = [word, ...variants];
    let result = context;
    for (const form of allForms) {
      const regex = new RegExp(`\\b(${form})\\b`, 'gi');
      result = result.replace(regex, '<mark>$1</mark>');
    }
    return result;
  }

  // Show filtered words toggle
  let showFiltered = $state(false);

  let books = $state<Book[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let libraryPath = $state<string | null>(null);

  // Analysis state
  let selectedBook = $state<Book | null>(null);
  let analyzing = $state(false);
  let analysisResult = $state<AnalysisResult | null>(null);
  let analysisError = $state<string | null>(null);
  let analysisProgress = $state<{ stage: string; progress: number; detail?: string } | null>(null);

  // Export state
  let exportedBooks = $state<Map<number, AnalysisResult>>(new Map());

  // Listen for progress events
  let unlistenProgress: (() => void) | null = null;

  onMount(async () => {
    unlistenProgress = await listen<{ book_id: number; stage: string; progress: number; detail?: string }>(
      "analysis-progress",
      (event) => {
        analysisProgress = {
          stage: event.payload.stage,
          progress: event.payload.progress,
          detail: event.payload.detail,
        };
      }
    );
  });

  onDestroy(() => {
    if (unlistenProgress) unlistenProgress();
  });

  // Animate books when they load
  $effect(() => {
    if (books.length > 0) {
      setTimeout(() => {
        animate(
          ".book-card",
          { opacity: [0, 1], y: [30, 0], scale: [0.9, 1] },
          { delay: stagger(0.05), duration: 0.4, easing: [0.22, 1, 0.36, 1] }
        );
      }, 50);
    }
  });

  // Animate modal
  $effect(() => {
    if (selectedBook) {
      setTimeout(() => {
        animate(".modal", { opacity: [0, 1], scale: [0.9, 1] }, { duration: 0.3, easing: [0.22, 1, 0.36, 1] });
      }, 10);
    }
  });

  // Animate word cards
  $effect(() => {
    if (analysisResult) {
      setTimeout(() => {
        animate(
          ".word-card",
          { opacity: [0, 1], x: [-20, 0] },
          { delay: stagger(0.03), duration: 0.3, easing: [0.22, 1, 0.36, 1] }
        );
      }, 50);
    }
  });

  async function selectLibrary() {
    const selected = await open({
      directory: true,
      title: "Select Calibre Library",
    });

    if (selected) {
      await loadLibrary(selected);
    }
  }

  async function loadLibrary(path: string) {
    loading = true;
    error = null;
    books = []; // Clear for animation
    try {
      books = await invoke("scan_library", { path });
      libraryPath = path;
    } catch (e) {
      error = String(e);
      books = [];
    } finally {
      loading = false;
    }
  }

  function getCoverUrl(coverPath: string | null): string {
    if (!coverPath) return "";
    return convertFileSrc(coverPath);
  }

  async function analyzeBook(book: Book) {
    selectedBook = book;
    analyzing = true;
    analysisError = null;
    analysisResult = null;
    analysisProgress = { stage: "Starting analysis...", progress: 0 };

    try {
      const result: AnalysisResult = await invoke("analyze_book", { bookId: book.id });
      analysisResult = result;
      exportedBooks.set(book.id, result);
    } catch (e) {
      analysisError = String(e);
    } finally {
      analyzing = false;
      analysisProgress = null;
    }
  }

  function closeModal() {
    selectedBook = null;
    analysisResult = null;
    analysisError = null;
  }

  async function exportToJson() {
    if (exportedBooks.size === 0) {
      alert("No books analyzed yet. Click on books to analyze them first.");
      return;
    }

    const path = await save({
      title: "Export Vocabulary Data",
      filters: [{ name: "JSON", extensions: ["json"] }],
      defaultPath: "lexis-export.json",
    });

    if (!path) return;

    const exportData = {
      version: 1,
      exported_at: new Date().toISOString(),
      books: Array.from(exportedBooks.entries()).map(([id, result]) => {
        const book = books.find(b => b.id === id);
        return {
          id: `calibre-${id}`,
          title: book?.title || "Unknown",
          author: book?.author || "Unknown",
          words: result.hard_words.map(w => ({
            word: w.word,
            frequency_score: w.frequency_score,
            contexts: w.contexts,
          })),
        };
      }),
    };

    try {
      await invoke("export_json", { path, content: JSON.stringify(exportData, null, 2) });
      alert(`Exported ${exportedBooks.size} book(s) to ${path}`);
    } catch (e) {
      alert(`Export failed: ${e}`);
    }
  }
</script>

<main class="container">
  <header>
    <h1>Lexis</h1>
    <p class="subtitle">Extract vocabulary from your ebook library</p>
  </header>

  <div class="controls">
    <button class="clay-btn primary" onclick={selectLibrary} disabled={loading}>
      {libraryPath ? "Change Library" : "Select Calibre Library"}
    </button>
    {#if libraryPath}
      <span class="library-path">{libraryPath}</span>
    {/if}
    {#if exportedBooks.size > 0}
      <button class="clay-btn success" onclick={exportToJson}>
        Export {exportedBooks.size} Book{exportedBooks.size > 1 ? "s" : ""}
      </button>
    {/if}
  </div>

  {#if loading}
    <div class="loading-container">
      <div class="clay-loader"></div>
      <p>Loading library...</p>
    </div>
  {:else if error}
    <div class="clay-card error-card">
      <p>{error}</p>
    </div>
  {:else if books.length > 0}
    <p class="status">
      {books.length} books found ({books.filter(b => b.has_epub).length} with EPUB)
      {#if exportedBooks.size > 0}
        <span class="analyzed-count">| {exportedBooks.size} analyzed</span>
      {/if}
    </p>
    <div class="book-grid">
      {#each books as book}
        <button
          class="book-card"
          class:no-epub={!book.has_epub}
          class:analyzed={exportedBooks.has(book.id)}
          onclick={() => book.has_epub && analyzeBook(book)}
          disabled={!book.has_epub}
          style="opacity: 0"
        >
          {#if book.cover_path}
            <img src={getCoverUrl(book.cover_path)} alt={book.title} class="cover" />
          {:else}
            <div class="no-cover">
              <span>{book.title.slice(0, 1)}</span>
            </div>
          {/if}
          <div class="book-info">
            <h3>{book.title}</h3>
            <p class="author">{book.author}</p>
            {#if !book.has_epub}
              <span class="badge warning">No EPUB</span>
            {:else if exportedBooks.has(book.id)}
              <span class="badge success">{exportedBooks.get(book.id)?.hard_words.length} words</span>
            {/if}
          </div>
        </button>
      {/each}
    </div>
  {:else if libraryPath}
    <div class="clay-card">
      <p>No books found</p>
    </div>
  {/if}
</main>

{#if selectedBook}
  <div class="modal-overlay" onclick={closeModal}>
    <div class="modal clay-card" onclick={(e) => e.stopPropagation()} style="opacity: 0">
      <header class="modal-header">
        <h2>{selectedBook.title}</h2>
        <p class="modal-author">{selectedBook.author}</p>
        <button class="close-btn clay-btn" onclick={closeModal}>×</button>
      </header>

      <div class="modal-content">
        {#if analyzing}
          <div class="loading-state">
            <div class="progress-container">
              <div class="progress-bar" style="width: {analysisProgress?.progress ?? 0}%"></div>
              <div class="progress-glow" style="width: {analysisProgress?.progress ?? 0}%"></div>
            </div>
            <p class="progress-stage">{analysisProgress?.stage ?? "Starting..."}</p>
            {#if analysisProgress?.detail}
              <p class="progress-detail">{analysisProgress.detail}</p>
            {/if}
            <p class="hint">This may take a moment for longer books</p>
          </div>
        {:else if analysisError}
          <div class="clay-card error-card">
            <p>{analysisError}</p>
          </div>
        {:else if analysisResult}
          <div class="analysis-summary">
            <div class="stat-card clay-card">
              <span class="stat-value">{analysisResult.word_count.toLocaleString()}</span>
              <span class="stat-label">total words</span>
            </div>
            <div class="stat-card clay-card">
              <span class="stat-value">{analysisResult.stats.total_candidates}</span>
              <span class="stat-label">candidates</span>
            </div>
            <div class="stat-card clay-card highlight">
              <span class="stat-value">{analysisResult.hard_words.length}</span>
              <span class="stat-label">hard words</span>
            </div>
          </div>

          {#if analysisResult.stats.filtered_by_ner.length > 0}
            <button
              class="filter-toggle clay-btn"
              onclick={() => showFiltered = !showFiltered}
            >
              {showFiltered ? 'Hide' : 'Show'} {analysisResult.stats.filtered_by_ner.length} filtered names
            </button>

            {#if showFiltered}
              <div class="filtered-words">
                {#each analysisResult.stats.filtered_by_ner.slice(0, 50) as word}
                  <span class="filtered-tag">{word}</span>
                {/each}
                {#if analysisResult.stats.filtered_by_ner.length > 50}
                  <span class="filtered-more">+{analysisResult.stats.filtered_by_ner.length - 50} more</span>
                {/if}
              </div>
            {/if}
          {/if}

          <div class="word-list">
            {#each analysisResult.hard_words as hardWord, i}
              <div class="word-card clay-card" style="opacity: 0">
                <div class="word-header">
                  <span class="rank">#{i + 1}</span>
                  <span class="word">{hardWord.word}</span>
                  {#if hardWord.variants.length > 0}
                    <span class="variants">({hardWord.variants.join(', ')})</span>
                  {/if}
                  <span class="count">{hardWord.count}×</span>
                </div>
                {#if hardWord.contexts.length > 0}
                  <p class="context">{@html `"${highlightWord(hardWord.contexts[0], hardWord.word, hardWord.variants)}"`}</p>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  :root {
    --bg-light: #f0e6ff;
    --bg-dark: #1a1625;
    --clay-light: #ffffff;
    --clay-dark: #2a2438;
    --primary: #a78bfa;
    --primary-dark: #7c3aed;
    --success: #6ee7b7;
    --success-dark: #059669;
    --warning: #fcd34d;
    --text-light: #1f1f1f;
    --text-dark: #f5f5f5;
    --text-muted-light: #6b7280;
    --text-muted-dark: #9ca3af;

    /* Claymorphism shadows */
    --clay-shadow-light:
      8px 8px 16px rgba(166, 139, 214, 0.25),
      -8px -8px 16px rgba(255, 255, 255, 0.8),
      inset 2px 2px 4px rgba(255, 255, 255, 0.6),
      inset -1px -1px 3px rgba(166, 139, 214, 0.15);
    --clay-shadow-dark:
      8px 8px 16px rgba(0, 0, 0, 0.4),
      -8px -8px 16px rgba(60, 50, 80, 0.3),
      inset 2px 2px 4px rgba(60, 50, 80, 0.4),
      inset -1px -1px 3px rgba(0, 0, 0, 0.2);
    --clay-shadow-pressed-light:
      inset 4px 4px 8px rgba(166, 139, 214, 0.3),
      inset -4px -4px 8px rgba(255, 255, 255, 0.5);
    --clay-shadow-pressed-dark:
      inset 4px 4px 8px rgba(0, 0, 0, 0.4),
      inset -4px -4px 8px rgba(60, 50, 80, 0.2);
  }

  :global(body) {
    font-family: 'SF Pro Rounded', -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    font-size: 16px;
    color: var(--text-light);
    background: linear-gradient(135deg, #f0e6ff 0%, #e0d4f7 50%, #d4c4f0 100%);
    min-height: 100vh;
    margin: 0;
    transition: background 0.3s, color 0.3s;
  }

  @media (prefers-color-scheme: dark) {
    :global(body) {
      color: var(--text-dark);
      background: linear-gradient(135deg, #1a1625 0%, #251e35 50%, #2a2145 100%);
    }
  }

  .container {
    max-width: 1200px;
    margin: 0 auto;
    padding: 2rem;
  }

  header {
    text-align: center;
    margin-bottom: 2.5rem;
  }

  h1 {
    margin: 0;
    font-size: 3rem;
    font-weight: 800;
    background: linear-gradient(135deg, var(--primary) 0%, var(--primary-dark) 100%);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    background-clip: text;
    text-shadow: 0 4px 12px rgba(167, 139, 250, 0.3);
  }

  .subtitle {
    color: var(--text-muted-light);
    margin: 0.5rem 0 0;
    font-size: 1.1rem;
    font-weight: 500;
  }

  @media (prefers-color-scheme: dark) {
    .subtitle {
      color: var(--text-muted-dark);
    }
  }

  /* Clay Card Base */
  .clay-card {
    background: var(--clay-light);
    border-radius: 24px;
    box-shadow: var(--clay-shadow-light);
    transition: all 0.3s cubic-bezier(0.22, 1, 0.36, 1);
  }

  @media (prefers-color-scheme: dark) {
    .clay-card {
      background: var(--clay-dark);
      box-shadow: var(--clay-shadow-dark);
    }
  }

  /* Clay Button */
  .clay-btn {
    padding: 0.875rem 1.75rem;
    font-size: 1rem;
    font-weight: 600;
    border: none;
    border-radius: 16px;
    background: var(--clay-light);
    color: var(--text-light);
    cursor: pointer;
    box-shadow: var(--clay-shadow-light);
    transition: all 0.2s cubic-bezier(0.22, 1, 0.36, 1);
  }

  .clay-btn:hover:not(:disabled) {
    transform: translateY(-2px);
    box-shadow:
      10px 10px 20px rgba(166, 139, 214, 0.3),
      -10px -10px 20px rgba(255, 255, 255, 0.9),
      inset 2px 2px 4px rgba(255, 255, 255, 0.6),
      inset -1px -1px 3px rgba(166, 139, 214, 0.15);
  }

  .clay-btn:active:not(:disabled) {
    transform: translateY(0);
    box-shadow: var(--clay-shadow-pressed-light);
  }

  .clay-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .clay-btn.primary {
    background: linear-gradient(135deg, var(--primary) 0%, var(--primary-dark) 100%);
    color: white;
  }

  .clay-btn.success {
    background: linear-gradient(135deg, var(--success) 0%, var(--success-dark) 100%);
    color: white;
  }

  @media (prefers-color-scheme: dark) {
    .clay-btn {
      background: var(--clay-dark);
      color: var(--text-dark);
      box-shadow: var(--clay-shadow-dark);
    }

    .clay-btn:hover:not(:disabled) {
      box-shadow:
        10px 10px 20px rgba(0, 0, 0, 0.5),
        -10px -10px 20px rgba(60, 50, 80, 0.4),
        inset 2px 2px 4px rgba(60, 50, 80, 0.4),
        inset -1px -1px 3px rgba(0, 0, 0, 0.2);
    }

    .clay-btn:active:not(:disabled) {
      box-shadow: var(--clay-shadow-pressed-dark);
    }
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 2rem;
    flex-wrap: wrap;
  }

  .library-path {
    font-size: 0.875rem;
    color: var(--text-muted-light);
    font-family: monospace;
    padding: 0.5rem 1rem;
    background: rgba(167, 139, 250, 0.1);
    border-radius: 12px;
  }

  @media (prefers-color-scheme: dark) {
    .library-path {
      color: var(--text-muted-dark);
      background: rgba(167, 139, 250, 0.15);
    }
  }

  .status {
    color: var(--text-muted-light);
    margin-bottom: 1.5rem;
    font-weight: 500;
  }

  .analyzed-count {
    color: var(--success-dark);
    font-weight: 600;
  }

  @media (prefers-color-scheme: dark) {
    .status {
      color: var(--text-muted-dark);
    }
    .analyzed-count {
      color: var(--success);
    }
  }

  .error-card {
    padding: 1.5rem;
    background: linear-gradient(135deg, #fecaca 0%, #fca5a5 100%);
    color: #991b1b;
  }

  @media (prefers-color-scheme: dark) {
    .error-card {
      background: linear-gradient(135deg, #450a0a 0%, #7f1d1d 100%);
      color: #fecaca;
    }
  }

  /* Loading */
  .loading-container {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 3rem;
    gap: 1rem;
  }

  .clay-loader {
    width: 60px;
    height: 60px;
    border-radius: 50%;
    background: linear-gradient(135deg, var(--primary) 0%, var(--primary-dark) 100%);
    box-shadow: var(--clay-shadow-light);
    animation: pulse 1.5s ease-in-out infinite;
  }

  @keyframes pulse {
    0%, 100% { transform: scale(1); opacity: 1; }
    50% { transform: scale(1.1); opacity: 0.8; }
  }

  @media (prefers-color-scheme: dark) {
    .clay-loader {
      box-shadow: var(--clay-shadow-dark);
    }
  }

  /* Book Grid */
  .book-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 1.5rem;
  }

  .book-card {
    display: flex;
    flex-direction: column;
    background: var(--clay-light);
    border-radius: 20px;
    overflow: hidden;
    box-shadow: var(--clay-shadow-light);
    transition: all 0.3s cubic-bezier(0.22, 1, 0.36, 1);
    padding: 0;
    text-align: left;
    cursor: pointer;
    border: none;
  }

  .book-card:hover:not(:disabled) {
    transform: translateY(-6px) scale(1.02);
    box-shadow:
      12px 12px 24px rgba(166, 139, 214, 0.35),
      -12px -12px 24px rgba(255, 255, 255, 0.9),
      inset 2px 2px 4px rgba(255, 255, 255, 0.6),
      inset -1px -1px 3px rgba(166, 139, 214, 0.15);
  }

  .book-card:active:not(:disabled) {
    transform: translateY(-2px) scale(1);
    box-shadow: var(--clay-shadow-pressed-light);
  }

  .book-card.no-epub {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .book-card.analyzed {
    border: 3px solid var(--success);
  }

  @media (prefers-color-scheme: dark) {
    .book-card {
      background: var(--clay-dark);
      box-shadow: var(--clay-shadow-dark);
    }

    .book-card:hover:not(:disabled) {
      box-shadow:
        12px 12px 24px rgba(0, 0, 0, 0.5),
        -12px -12px 24px rgba(60, 50, 80, 0.4),
        inset 2px 2px 4px rgba(60, 50, 80, 0.4),
        inset -1px -1px 3px rgba(0, 0, 0, 0.2);
    }

    .book-card:active:not(:disabled) {
      box-shadow: var(--clay-shadow-pressed-dark);
    }
  }

  .cover {
    width: 100%;
    aspect-ratio: 2/3;
    object-fit: cover;
    border-radius: 16px 16px 0 0;
  }

  .no-cover {
    width: 100%;
    aspect-ratio: 2/3;
    background: linear-gradient(135deg, var(--primary) 0%, var(--primary-dark) 100%);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 3.5rem;
    color: white;
    font-weight: 800;
    border-radius: 16px 16px 0 0;
  }

  .book-info {
    padding: 1rem;
  }

  .book-info h3 {
    margin: 0;
    font-size: 0.95rem;
    font-weight: 700;
    line-height: 1.3;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .author {
    margin: 0.25rem 0 0;
    font-size: 0.8rem;
    color: var(--text-muted-light);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  @media (prefers-color-scheme: dark) {
    .author {
      color: var(--text-muted-dark);
    }
  }

  .badge {
    display: inline-block;
    margin-top: 0.5rem;
    padding: 0.25rem 0.75rem;
    font-size: 0.7rem;
    font-weight: 600;
    border-radius: 20px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .badge.warning {
    background: linear-gradient(135deg, var(--warning) 0%, #f59e0b 100%);
    color: #78350f;
  }

  .badge.success {
    background: linear-gradient(135deg, var(--success) 0%, var(--success-dark) 100%);
    color: white;
  }

  /* Modal */
  .modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    backdrop-filter: blur(8px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    padding: 1rem;
  }

  .modal {
    width: 90%;
    max-width: 600px;
    max-height: 85vh;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    padding: 0;
  }

  .modal-header {
    padding: 1.5rem 2rem;
    border-bottom: 1px solid rgba(167, 139, 250, 0.2);
    position: relative;
  }

  .modal-header h2 {
    margin: 0;
    padding-right: 3rem;
    font-weight: 700;
    font-size: 1.5rem;
  }

  .modal-author {
    margin: 0.25rem 0 0;
    color: var(--text-muted-light);
    font-weight: 500;
  }

  @media (prefers-color-scheme: dark) {
    .modal-author {
      color: var(--text-muted-dark);
    }
  }

  .close-btn {
    position: absolute;
    top: 1rem;
    right: 1rem;
    width: 2.5rem;
    height: 2.5rem;
    padding: 0;
    font-size: 1.5rem;
    line-height: 1;
    border-radius: 12px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .modal-content {
    padding: 1.5rem 2rem;
    overflow-y: auto;
    flex: 1;
  }

  .loading-state {
    text-align: center;
    padding: 2rem 1rem;
  }

  .progress-container {
    width: 100%;
    height: 12px;
    background: rgba(167, 139, 250, 0.2);
    border-radius: 12px;
    overflow: hidden;
    margin-bottom: 1rem;
    position: relative;
  }

  .progress-bar {
    height: 100%;
    background: linear-gradient(90deg, var(--primary) 0%, var(--primary-dark) 100%);
    border-radius: 12px;
    transition: width 0.3s ease;
    position: relative;
  }

  .progress-glow {
    position: absolute;
    top: 0;
    left: 0;
    height: 100%;
    background: linear-gradient(90deg, transparent, rgba(255, 255, 255, 0.4), transparent);
    border-radius: 12px;
    animation: shimmer 1.5s infinite;
  }

  @keyframes shimmer {
    0% { transform: translateX(-100%); }
    100% { transform: translateX(100%); }
  }

  .progress-stage {
    font-weight: 600;
    margin-bottom: 0.25rem;
    color: var(--primary-dark);
  }

  @media (prefers-color-scheme: dark) {
    .progress-stage {
      color: var(--primary);
    }
  }

  .progress-detail {
    font-size: 0.9rem;
    font-weight: 500;
    color: var(--text-muted-light);
    margin-bottom: 0.5rem;
    font-family: 'SF Mono', ui-monospace, monospace;
    background: rgba(167, 139, 250, 0.1);
    padding: 0.375rem 0.75rem;
    border-radius: 8px;
    display: inline-block;
  }

  @media (prefers-color-scheme: dark) {
    .progress-detail {
      color: var(--text-muted-dark);
      background: rgba(167, 139, 250, 0.15);
    }
  }

  .hint {
    color: var(--text-muted-light);
    font-size: 0.875rem;
  }

  @media (prefers-color-scheme: dark) {
    .hint {
      color: var(--text-muted-dark);
    }
  }

  .analysis-summary {
    display: flex;
    gap: 1rem;
    margin-bottom: 1.5rem;
  }

  .stat-card {
    flex: 1;
    padding: 1rem;
    text-align: center;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .stat-card.highlight {
    background: linear-gradient(135deg, var(--primary) 0%, var(--primary-dark) 100%);
  }

  .stat-card.highlight .stat-value {
    background: none;
    -webkit-text-fill-color: white;
    color: white;
  }

  .stat-card.highlight .stat-label {
    color: rgba(255, 255, 255, 0.8);
  }

  .stat-value {
    font-size: 1.75rem;
    font-weight: 800;
    background: linear-gradient(135deg, var(--primary) 0%, var(--primary-dark) 100%);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    background-clip: text;
  }

  .stat-label {
    font-size: 0.75rem;
    color: var(--text-muted-light);
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  @media (prefers-color-scheme: dark) {
    .stat-label {
      color: var(--text-muted-dark);
    }
  }

  .filter-toggle {
    width: 100%;
    margin-bottom: 1rem;
    padding: 0.625rem 1rem;
    font-size: 0.875rem;
    background: rgba(167, 139, 250, 0.1);
  }

  .filtered-words {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    margin-bottom: 1.5rem;
    padding: 1rem;
    background: rgba(239, 68, 68, 0.1);
    border-radius: 12px;
  }

  .filtered-tag {
    font-size: 0.75rem;
    padding: 0.25rem 0.5rem;
    background: rgba(239, 68, 68, 0.2);
    color: #dc2626;
    border-radius: 6px;
    font-weight: 500;
  }

  @media (prefers-color-scheme: dark) {
    .filtered-tag {
      background: rgba(239, 68, 68, 0.3);
      color: #fca5a5;
    }
  }

  .filtered-more {
    font-size: 0.75rem;
    color: var(--text-muted-light);
    font-style: italic;
  }

  .word-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .word-card {
    padding: 1rem 1.25rem;
    border-radius: 16px;
  }

  .word-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .rank {
    color: var(--text-muted-light);
    font-size: 0.75rem;
    font-weight: 600;
    min-width: 2rem;
  }

  .variants {
    font-size: 0.75rem;
    color: var(--text-muted-light);
    font-style: italic;
  }

  @media (prefers-color-scheme: dark) {
    .variants {
      color: var(--text-muted-dark);
    }
  }

  @media (prefers-color-scheme: dark) {
    .rank {
      color: var(--text-muted-dark);
    }
  }

  .word {
    font-weight: 700;
    font-size: 1.1rem;
    flex: 1;
    color: var(--primary-dark);
  }

  @media (prefers-color-scheme: dark) {
    .word {
      color: var(--primary);
    }
  }

  .count {
    color: var(--text-muted-light);
    font-size: 0.8rem;
    font-weight: 600;
    background: rgba(167, 139, 250, 0.15);
    padding: 0.25rem 0.5rem;
    border-radius: 8px;
  }

  @media (prefers-color-scheme: dark) {
    .count {
      color: var(--text-muted-dark);
    }
  }

  .context {
    margin: 0.75rem 0 0;
    font-size: 0.875rem;
    color: var(--text-muted-light);
    font-style: italic;
    line-height: 1.6;
    padding-left: 2.5rem;
  }

  .context :global(mark) {
    background: linear-gradient(135deg, rgba(167, 139, 250, 0.3) 0%, rgba(124, 58, 237, 0.3) 100%);
    color: var(--primary-dark);
    font-style: normal;
    font-weight: 600;
    padding: 0.125rem 0.25rem;
    border-radius: 4px;
  }

  @media (prefers-color-scheme: dark) {
    .context {
      color: var(--text-muted-dark);
    }

    .context :global(mark) {
      background: linear-gradient(135deg, rgba(167, 139, 250, 0.4) 0%, rgba(124, 58, 237, 0.4) 100%);
      color: var(--primary);
    }
  }
</style>

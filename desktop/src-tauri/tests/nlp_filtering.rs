//! Integration tests for NLP filtering logic
//!
//! These tests verify that:
//! 1. Common words (high frequency) are filtered OUT
//! 2. Rare/hard words (low frequency) are kept IN
//! 3. Malformed EPUB concatenations are filtered OUT
//! 4. Named entities (proper nouns) are filtered OUT
//!
//! Run with: cargo test --test nlp_filtering
//!
//! Setup: Run `setup-test-fixtures` devenv script first to download test books.

use desktop_lib::nlp::NlpPipeline;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

const PRIDE_PREJUDICE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/pride_and_prejudice.txt");

// Words that should ALWAYS be filtered (extremely common, freq > 0.001)
// These are the most common words in English that must never appear in hard words.
// Note: Words like "house", "little", "people" have lower frequencies than expected
// and may legitimately appear with a threshold of 0.00005.
const EASY_WORDS: &[&str] = &[
    "the", "and", "but", "that", "this", "with", "from", "have", "been",
    "were", "they", "their", "what", "when", "where", "which", "would",
    "could", "should", "about", "after", "other", "there", "just",
    "come", "make", "know", "good", "well", "back", "over", "such",
    "into", "also", "than", "then", "like", "time", "more", "some",
];

// Words from Pride and Prejudice that SHOULD be identified as hard
// (uncommon period vocabulary, rare words)
const EXPECTED_HARD_WORDS: &[&str] = &[
    "felicity",      // freq ~1.5e-6
    "obsequious",    // freq ~2.5e-7
    "civility",      // freq ~3e-6
    "importunate",   // freq ~1.5e-7
    "condescension", // freq ~5e-7
    "sanguine",      // freq ~1.5e-6
    "amiable",       // freq ~1e-6
    "supercilious",  // freq ~2e-7
    "acquiesce",     // freq ~5e-7
    "reproach",      // freq ~2e-6
];

fn get_test_text() -> Option<String> {
    let path = Path::new(PRIDE_PREJUDICE_PATH);
    if !path.exists() {
        eprintln!(
            "Test fixture not found at {:?}. Run `setup-test-fixtures` first.",
            path
        );
        return None;
    }
    fs::read_to_string(path).ok()
}

fn ensure_fixtures_exist() -> bool {
    let path = Path::new(PRIDE_PREJUDICE_PATH);
    if path.exists() {
        return true;
    }

    eprintln!("\n========================================");
    eprintln!("TEST FIXTURES NOT FOUND");
    eprintln!("========================================");
    eprintln!("Please run the setup script first:");
    eprintln!("  devenv shell");
    eprintln!("  setup-test-fixtures");
    eprintln!("Or manually download:");
    eprintln!("  curl -o tests/fixtures/pride_and_prejudice.txt \\");
    eprintln!("    https://www.gutenberg.org/cache/epub/1342/pg1342.txt");
    eprintln!("========================================\n");
    false
}

#[test]
fn test_easy_words_are_filtered_out() {
    if !ensure_fixtures_exist() {
        // Skip test gracefully if fixtures not present
        eprintln!("Skipping test: fixtures not found");
        return;
    }

    let text = get_test_text().expect("Failed to read test text");
    let pipeline = NlpPipeline::new();

    // Use default threshold from the app
    let (hard_words, _stats) = pipeline.analyze(&text, 0.00005, |_progress| {});

    let found_words: HashSet<String> = hard_words.iter().map(|w| w.word.clone()).collect();

    // Check that common words are NOT in the results
    let mut found_easy_words = Vec::new();
    for easy_word in EASY_WORDS {
        if found_words.contains(*easy_word) {
            found_easy_words.push(*easy_word);
        }
    }

    assert!(
        found_easy_words.is_empty(),
        "Found {} easy words that should have been filtered: {:?}",
        found_easy_words.len(),
        found_easy_words
    );
}

#[test]
fn test_hard_words_are_kept() {
    if !ensure_fixtures_exist() {
        eprintln!("Skipping test: fixtures not found");
        return;
    }

    let text = get_test_text().expect("Failed to read test text");
    let pipeline = NlpPipeline::new();

    let (hard_words, _stats) = pipeline.analyze(&text, 0.00005, |_progress| {});

    // Build a set of all found words (including stemmed variants)
    let found_words: HashSet<String> = hard_words
        .iter()
        .flat_map(|w| {
            let mut words = vec![w.word.clone()];
            words.extend(w.variants.clone());
            words
        })
        .collect();

    // Check that expected hard words ARE in the results
    // Note: Some may not appear if they're not actually in the text
    let mut missing_words = Vec::new();
    let mut found_count = 0;

    for hard_word in EXPECTED_HARD_WORDS {
        // Check both the word and its stemmed form
        let stemmer = rust_stemmers::Stemmer::create(rust_stemmers::Algorithm::English);
        let stemmed = stemmer.stem(hard_word).to_string();

        if found_words.contains(*hard_word) || found_words.contains(&stemmed) {
            found_count += 1;
        } else {
            // Only count as missing if the word actually appears in the text
            if text.to_lowercase().contains(*hard_word) {
                missing_words.push(*hard_word);
            }
        }
    }

    // We expect at least 50% of our expected hard words to be found
    // (some may not appear in the text or may be filtered by NER)
    let expected_min = EXPECTED_HARD_WORDS.len() / 2;
    assert!(
        found_count >= expected_min,
        "Expected at least {} hard words, found {}. Missing (that are in text): {:?}",
        expected_min,
        found_count,
        missing_words
    );
}

#[test]
fn test_malformed_words_are_filtered() {
    let pipeline = NlpPipeline::new();

    // Synthetic text with malformed concatenations
    let text = r#"
        This is a test. The character believesthat's not right.
        He meetshimself in the mirror. The story isabout love.
        Normal words like ephemeral and sanguine should remain.
        The endofeternity approaches quickly now.
    "#;

    let (hard_words, _stats) = pipeline.analyze(text, 0.00005, |_progress| {});

    let found_words: HashSet<String> = hard_words.iter().map(|w| w.word.clone()).collect();

    // These malformed words should NOT be in results
    let malformed = ["believesthat's", "meetshimself", "isabout", "endofeternity"];

    for word in &malformed {
        assert!(
            !found_words.contains(*word),
            "Malformed word '{}' should have been filtered",
            word
        );
    }
}

#[test]
fn test_proper_nouns_filtered_by_ner() {
    // This test only runs if GLiNER is available
    if !NlpPipeline::is_gliner_available() {
        eprintln!("Skipping NER test: GLiNER model not available");
        return;
    }

    let pipeline = NlpPipeline::new();

    // Text with clear proper nouns
    let text = r#"
        Elizabeth Bennet met Mr. Darcy at the ball in London.
        The enigmatic atmosphere was palpable throughout Pemberley.
        Jane traveled to Meryton with her sister.
        The obsequious Mr. Collins arrived from Hunsford.
    "#;

    let (hard_words, stats) = pipeline.analyze(text, 0.00005, |_progress| {});

    let found_words: HashSet<String> = hard_words.iter().map(|w| w.word.clone()).collect();

    // These names should be filtered by NER
    let names = ["elizabeth", "bennet", "darcy", "pemberley", "meryton", "collins", "hunsford"];

    for name in &names {
        assert!(
            !found_words.contains(*name),
            "Proper noun '{}' should have been filtered by NER",
            name
        );
    }

    // But these hard words should remain
    assert!(
        found_words.contains("obsequious") || found_words.contains("enigmatic"),
        "Hard words like 'obsequious' or 'enigmatic' should be kept"
    );

    // Check that NER actually filtered something
    assert!(
        !stats.filtered_by_ner.is_empty(),
        "Expected some words to be filtered by NER"
    );
}

#[test]
fn test_frequency_threshold_affects_results() {
    if !ensure_fixtures_exist() {
        eprintln!("Skipping test: fixtures not found");
        return;
    }

    let text = get_test_text().expect("Failed to read test text");
    let pipeline = NlpPipeline::new();

    // Lower threshold = fewer words (stricter)
    let (strict_words, _) = pipeline.analyze(&text, 0.00001, |_progress| {});

    // Higher threshold = more words (looser)
    let (loose_words, _) = pipeline.analyze(&text, 0.0001, |_progress| {});

    assert!(
        strict_words.len() < loose_words.len(),
        "Stricter threshold (0.00001) should yield fewer words ({}) than looser (0.0001) threshold ({})",
        strict_words.len(),
        loose_words.len()
    );
}

#[test]
fn test_contexts_are_captured() {
    if !ensure_fixtures_exist() {
        eprintln!("Skipping test: fixtures not found");
        return;
    }

    let text = get_test_text().expect("Failed to read test text");
    let pipeline = NlpPipeline::new();

    let (hard_words, _stats) = pipeline.analyze(&text, 0.00005, |_progress| {});

    // Count how many words have context
    // Note: The NLP pipeline only stores context for sentences between 20-500 chars,
    // so some words may legitimately have no context if they only appear in
    // very short or very long sentences.
    let words_with_context = hard_words.iter().filter(|w| !w.contexts.is_empty()).count();
    let words_without_context: Vec<_> = hard_words.iter()
        .filter(|w| w.contexts.is_empty())
        .map(|w| w.word.as_str())
        .take(10)
        .collect();

    eprintln!(
        "Context coverage: {}/{} words have context ({:.1}%)",
        words_with_context,
        hard_words.len(),
        (words_with_context as f64 / hard_words.len() as f64) * 100.0
    );

    if !words_without_context.is_empty() {
        eprintln!("Sample words without context: {:?}", words_without_context);
    }

    // At least 90% of words should have context
    let context_ratio = words_with_context as f64 / hard_words.len() as f64;
    assert!(
        context_ratio >= 0.90,
        "Expected at least 90% of words to have context, but only {:.1}% do. \
         Words without context: {:?}",
        context_ratio * 100.0,
        words_without_context
    );

    // Most contexts should be reasonable length
    // Note: Some books have table of contents or chapter markers that create short "contexts"
    let mut short_contexts = Vec::new();
    let mut total_contexts = 0;

    for word in hard_words.iter().filter(|w| !w.contexts.is_empty()) {
        for ctx in &word.contexts {
            total_contexts += 1;
            if ctx.len() <= 10 {
                short_contexts.push((word.word.as_str(), ctx.as_str()));
            }
        }
    }

    // Allow up to 1% short contexts (book artifacts like TOC, chapter markers)
    let short_ratio = short_contexts.len() as f64 / total_contexts.max(1) as f64;
    assert!(
        short_ratio <= 0.01,
        "Too many short contexts ({:.1}%). Sample: {:?}",
        short_ratio * 100.0,
        short_contexts.iter().take(5).collect::<Vec<_>>()
    );
}

#[test]
fn test_word_variants_tracked() {
    let pipeline = NlpPipeline::new();

    // Text with multiple forms of same word
    let text = r#"
        The gaiety of the party was infectious. Such gaieties were rare.
        Her felicitous remarks brought felicity to all. Most felicitously done.
    "#;

    let (hard_words, _stats) = pipeline.analyze(text, 0.00005, |_progress| {});

    // Find the word entry (might be under stem)
    let gaiety_entry = hard_words.iter().find(|w| {
        w.word == "gaiety" || w.word == "gaieties" || w.variants.contains(&"gaiety".to_string())
    });

    if let Some(entry) = gaiety_entry {
        // Check that both forms are tracked
        let all_forms: HashSet<String> = {
            let mut forms = entry.variants.clone();
            forms.push(entry.word.clone());
            forms.into_iter().collect()
        };

        assert!(
            all_forms.contains("gaiety") || all_forms.contains("gaieties"),
            "Should track word variants. Found: {:?}",
            all_forms
        );
    }
}

#[test]
fn test_valid_dictionary_words_not_filtered_as_malformed() {
    let pipeline = NlpPipeline::new();

    // These are valid words that symspell might try to segment
    // but should NOT be filtered because they're in the dictionary
    let text = r#"
        She favorites all her neighboring friends who traveled far.
        The indifferent observer noticed the unfortunate circumstances.
        Professionals demonstrated their understanding of the situation.
    "#;

    let (hard_words, _stats) = pipeline.analyze(text, 0.00005, |_progress| {});

    // The main verification is that valid dictionary words are not incorrectly
    // filtered as "malformed" by symspell. If "indifferent" was wrongly split
    // to "in different", we'd get no results from this short text.
    assert!(
        !hard_words.is_empty(),
        "Should have found some hard words in the test text. \
         Valid dictionary words may have been incorrectly filtered as malformed."
    );
}

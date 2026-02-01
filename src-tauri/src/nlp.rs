use crate::resources;
use gliner::model::{GLiNER, input::text::TextInput, pipeline::span::SpanMode};
use orp::params::RuntimeParameters;

#[cfg(target_os = "macos")]
use ort::execution_providers::CoreMLExecutionProvider;
use rust_stemmers::{Algorithm, Stemmer};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use symspell::{AsciiStringStrategy, SymSpell};
use unicode_segmentation::UnicodeSegmentation;
use wordfreq::WordFreq;
use wordfreq_model::{load_wordfreq, ModelKind};

#[derive(Debug, Serialize, Clone)]
pub struct HardWord {
    pub word: String,
    pub frequency_score: f64,
    pub contexts: Vec<String>,
    pub count: usize,
    pub variants: Vec<String>, // All forms found (gaiety, gaieties, etc.)
}

#[derive(Debug, Serialize, Clone)]
pub struct AnalysisProgress {
    pub stage: String,
    pub progress: u8,
    pub detail: Option<String>,
    pub sample_words: Option<Vec<SampleWord>>,
}

#[derive(Debug, Serialize, Clone)]
pub struct SampleWord {
    pub word: String,
    pub is_entity: bool, // true = will be filtered, false = kept
}

#[derive(Debug, Serialize, Clone)]
pub struct AnalysisStats {
    pub total_candidates: usize,
    pub filtered_by_ner: Vec<String>,
    pub hard_words_count: usize,
}

static GLINER_MODEL: OnceLock<Option<GLiNER<SpanMode>>> = OnceLock::new();
static SYMSPELL: OnceLock<Option<SymSpell<AsciiStringStrategy>>> = OnceLock::new();

pub struct NlpPipeline {
    wordfreq: WordFreq,
    stemmer: Stemmer,
}

impl NlpPipeline {
    pub fn new() -> Self {
        let wordfreq = load_wordfreq(ModelKind::LargeEn).expect("Failed to load wordfreq model");
        let stemmer = Stemmer::create(Algorithm::English);
        Self { wordfreq, stemmer }
    }

    /// Stem a word (input must be lowercase)
    fn stem(&self, word: &str) -> String {
        self.stemmer.stem(word).to_string()
    }

    /// Check if a word looks like concatenated words (e.g., "believethat's")
    /// Returns true if the word should be filtered out as malformed
    ///
    /// Key insight: Only check words NOT in wordfreq dictionary.
    /// Words like "favorites", "traveled", "neighboring" ARE valid words
    /// and should NOT be filtered even if symspell can segment them.
    fn is_malformed_word(&self, word: &str) -> bool {
        // Skip short words - they can't be meaningful concatenations
        if word.len() < 10 {
            return false;
        }

        // Handle words with apostrophes by checking the part before
        let check_word = if let Some(pos) = word.find('\'') {
            &word[..pos]
        } else {
            word
        };

        // CRITICAL: If the word (or its base) is in the dictionary, it's valid!
        // This prevents filtering real words like "favorites", "neighboring", "traveled"
        if self.wordfreq.word_frequency(check_word) > 0.0 {
            return false;
        }

        // Also check stemmed form
        let stemmed = self.stem(check_word);
        if self.wordfreq.word_frequency(&stemmed) > 0.0 {
            return false;
        }

        // Only for words NOT in dictionary: try symspell segmentation
        if let Some(symspell) = get_symspell() {
            if check_word.len() >= 8 {
                let segmentation = symspell.word_segmentation(check_word, 2);
                let segments: Vec<&str> = segmentation.segmented_string.split_whitespace().collect();

                // If segmentation found multiple words, check if it makes sense
                if segments.len() >= 2 {
                    // All segments must be at least 3 chars and be real words
                    let all_valid = segments.iter().all(|s| {
                        s.len() >= 3 && self.wordfreq.word_frequency(s) > 0.0
                    });

                    if all_valid {
                        eprintln!("Filtering malformed word '{}' -> '{}'", word, segmentation.segmented_string);
                        return true;
                    }
                }
            }
        }

        // Fallback: heuristic for obvious concatenations with common words
        let common_suffixes = ["that's", "that", "the", "this", "they"];

        for suffix in &common_suffixes {
            if word.ends_with(suffix) && word.len() > suffix.len() + 4 {
                let prefix = &word[..word.len() - suffix.len()];
                if prefix.len() >= 4 && self.wordfreq.word_frequency(prefix) > 0.0 {
                    eprintln!("Filtering malformed word '{}' (heuristic: '{}' + '{}')", word, prefix, suffix);
                    return true;
                }
            }
        }

        false
    }

    pub fn is_gliner_available() -> bool {
        resources::is_gliner_available()
    }

    fn get_gliner(&self) -> Option<&GLiNER<SpanMode>> {
        GLINER_MODEL.get_or_init(|| {
            let model_dir = resources::get_gliner_dir();
            let tokenizer_path = model_dir.join("tokenizer.json");
            let model_path = model_dir.join("model.onnx");

            if !tokenizer_path.exists() || !model_path.exists() {
                eprintln!("GLiNER model not found at {:?}", model_dir);
                eprintln!("Run resource download to fetch the model automatically");
                return None;
            }

            // Configure runtime with CoreML on macOS for better performance
            #[cfg(target_os = "macos")]
            let runtime_params = RuntimeParameters::default()
                .with_threads(8)
                .with_execution_providers([CoreMLExecutionProvider::default().build()]);

            #[cfg(target_os = "macos")]
            eprintln!("GLiNER runtime: CoreML execution provider configured");

            #[cfg(not(target_os = "macos"))]
            let runtime_params = RuntimeParameters::default().with_threads(8);

            #[cfg(not(target_os = "macos"))]
            eprintln!("GLiNER runtime: default CPU execution provider configured");

            match GLiNER::<SpanMode>::new(
                Default::default(),
                runtime_params,
                tokenizer_path,
                model_path,
            ) {
                Ok(model) => {
                    eprintln!("GLiNER model loaded successfully");
                    Some(model)
                }
                Err(e) => {
                    eprintln!("Failed to load GLiNER model: {}", e);
                    None
                }
            }
        }).as_ref()
    }

    /// Extract entities from a limited set of sentences (for filtering hard words)
    fn extract_entities_from_sentences<F>(
        &self,
        sentences: &[&str],
        mut on_progress: F,
    ) -> HashSet<String>
    where
        F: FnMut(usize, usize, usize, &[String]), // (sentences_processed, total_sentences, entities_found, recent_entities)
    {
        let mut entities = HashSet::new();
        let mut recent_entities: Vec<String> = Vec::new();

        let Some(gliner) = self.get_gliner() else {
            return entities;
        };

        if sentences.is_empty() {
            return entities;
        }

        // Filter and prepare chunks
        let chunks: Vec<&str> = sentences
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && s.len() < 512)
            .collect();

        if chunks.is_empty() {
            return entities;
        }

        let total_sentences = chunks.len();
        eprintln!("Running GLiNER on {} sentences...", total_sentences);

        // Process in smaller batches for better CoreML utilization
        let batch_size = 64;
        let mut processed = 0;

        let mut total_infer_ms: u128 = 0;
        for (batch_idx, batch) in chunks.chunks(batch_size).enumerate() {
            let input = match TextInput::from_str(
                batch,
                &["person", "location", "organization", "country", "city"],
            ) {
                Ok(input) => input,
                Err(e) => {
                    eprintln!("Failed to create GLiNER input: {}", e);
                    processed += batch.len();
                    continue;
                }
            };

            // Clear recent for this batch
            recent_entities.clear();

            let infer_start = std::time::Instant::now();
            match gliner.inference(input) {
                Ok(output) => {
                    for spans in output.spans.iter() {
                        for span in spans.iter() {
                            let entity_text = span.text().to_lowercase();
                            if entities.insert(entity_text.clone()) {
                                // New entity found
                                recent_entities.push(entity_text.clone());
                            }
                            // Also add individual words from multi-word entities
                            for word in entity_text.split_whitespace() {
                                if entities.insert(word.to_string()) {
                                    recent_entities.push(word.to_string());
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("GLiNER inference error: {}", e);
                }
            }
            let infer_elapsed = infer_start.elapsed();
            total_infer_ms += infer_elapsed.as_millis();
            if batch_idx == 0 {
                eprintln!(
                    "GLiNER first batch inference: {} ms for {} sentences (batch size {})",
                    infer_elapsed.as_millis(),
                    batch.len(),
                    batch_size
                );
            }

            processed += batch.len();
            // Report progress after processing each batch with recent entities
            on_progress(processed, total_sentences, entities.len(), &recent_entities);
        }

        if total_sentences > 0 {
            let avg_ms = total_infer_ms as f64 / total_sentences as f64;
            eprintln!(
                "GLiNER total inference time: {} ms for {} sentences (avg {:.2} ms/sentence)",
                total_infer_ms,
                total_sentences,
                avg_ms
            );
        }

        eprintln!("GLiNER found {} unique entities", entities.len());
        entities
    }

    pub fn analyze<F>(&self, text: &str, frequency_threshold: f32, mut on_progress: F) -> (Vec<HardWord>, AnalysisStats)
    where
        F: FnMut(AnalysisProgress),
    {
        // Split into sentences for context
        let sentences: Vec<&str> = text
            .split(|c| c == '.' || c == '!' || c == '?')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        on_progress(AnalysisProgress {
            stage: "Analyzing text".to_string(),
            progress: 20,
            detail: Some(format!("{} sentences", sentences.len())),
            sample_words: None,
        });

        eprintln!("Processing {} sentences...", sentences.len());

        // FIRST PASS: Collect word counts and identify hard word CANDIDATES using wordfreq
        // This is fast and filters out most words before we even touch GLiNER
        // Key is stemmed form, value is (count, contexts, is_proper_noun_candidate, original_forms)
        let mut word_data: HashMap<String, (usize, Vec<String>, bool, HashSet<String>, HashSet<String>)> = HashMap::new();

        for sentence in &sentences {
            let words: Vec<&str> = sentence.unicode_words().collect();

            for word in &words {
                let lower = word.to_lowercase();

                // Skip short words
                if lower.len() < 3 {
                    continue;
                }

                // Skip words with numbers
                if lower.chars().any(|c| c.is_numeric()) {
                    continue;
                }

                // Stem the word for grouping (running, runs, run -> run)
                let stemmed = self.stem(&lower);

                // Check if likely proper noun (will need NER verification)
                let is_proper = is_likely_proper_noun(word, sentence);

                let entry = word_data.entry(stemmed).or_insert((0, Vec::new(), false, HashSet::new(), HashSet::new()));
                entry.0 += 1;
                if is_proper {
                    entry.2 = true; // Mark as needing NER check
                }
                entry.3.insert(lower); // Track original forms

                // Store context sentence (no limit - UI will handle display)
                if sentence.len() > 20 && sentence.len() < 500 {
                    let context = format!("{}.", sentence);
                    if !entry.1.contains(&context) {
                        entry.1.push(context.clone());
                    }
                    if is_proper {
                        entry.4.insert(context);
                    }
                }
            }
        }

        // Filter to get hard word candidates based on frequency
        // Use stemmed form for frequency lookup, but try original forms too
        let candidates: Vec<(String, usize, Vec<String>, bool, HashSet<String>, HashSet<String>)> = word_data
            .into_iter()
            .filter_map(|(stemmed, (count, contexts, needs_ner, original_forms, ner_contexts))| {
                // Filter out malformed words (EPUB parsing errors like "believethat's")
                for form in &original_forms {
                    if self.is_malformed_word(form) {
                        return None;
                    }
                }

                // Try stemmed form first, then original forms
                let mut freq = self.wordfreq.word_frequency(&stemmed);
                if freq == 0.0 {
                    // Stemmed form not in dictionary, try original forms
                    for original in &original_forms {
                        let orig_freq = self.wordfreq.word_frequency(original);
                        if orig_freq > freq {
                            freq = orig_freq;
                        }
                    }
                }

                // Filter out very common words and words not in dictionary
                if freq > frequency_threshold || freq == 0.0 {
                    return None;
                }

                Some((stemmed, count, contexts, needs_ner, original_forms, ner_contexts))
            })
            .collect();

        eprintln!("Found {} hard word candidates after wordfreq filtering", candidates.len());

        // SECOND PASS: Only run GLiNER on sentences containing candidates that need NER verification
        // This is MUCH faster than running on the entire book
        let sentences_needing_ner: Vec<&str> = candidates
            .iter()
            .filter(|(_, _, _, needs_ner, _, _)| *needs_ner)
            .flat_map(|(_, _, _, _, _, ner_contexts)| {
                ner_contexts.iter().map(|c| c.trim_end_matches('.'))
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        let total_candidates = candidates.len();
        let named_entities = if !sentences_needing_ner.is_empty() {
            let total_ner_sentences = sentences_needing_ner.len();
            eprintln!("Running NER on {} sentences containing proper noun candidates...", total_ner_sentences);

            // Get sample rare words (sorted by frequency, rarest first) to show in progress
            let rare_word_samples: Vec<String> = {
                let mut sorted_candidates: Vec<_> = candidates.iter()
                    .map(|(_, _, _, _, forms, _)| {
                        let form = forms.iter().next().cloned().unwrap_or_default();
                        let freq = self.wordfreq.word_frequency(&form);
                        (form, freq)
                    })
                    .filter(|(_, freq)| *freq > 0.0) // Must be in dictionary
                    .collect();
                sorted_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                sorted_candidates.into_iter().map(|(w, _)| w).take(20).collect()
            };

            on_progress(AnalysisProgress {
                stage: "Filtering names & places".to_string(),
                progress: 40,
                detail: Some(format!("0/{} sentences", total_ner_sentences)),
                sample_words: None,
            });

            let mut sample_index = 0usize;
            self.extract_entities_from_sentences(&sentences_needing_ner, |processed, total, found, recent_entities| {
                let ner_progress = 40 + (processed * 40 / total.max(1)) as u8;

                // Build sample words: recent entities (filtered) + rare candidates (kept)
                let mut samples: Vec<SampleWord> = Vec::new();

                // Add recent entities found this batch (these will be filtered)
                for entity in recent_entities.iter().take(4) {
                    samples.push(SampleWord {
                        word: entity.clone(),
                        is_entity: true,
                    });
                }

                // Add some rare candidates (rotating through the list)
                for i in 0..4 {
                    let idx = (sample_index + i) % rare_word_samples.len().max(1);
                    if let Some(word) = rare_word_samples.get(idx) {
                        if !recent_entities.contains(word) {
                            samples.push(SampleWord {
                                word: word.clone(),
                                is_entity: false,
                            });
                        }
                    }
                }
                sample_index = (sample_index + 2) % rare_word_samples.len().max(1);

                on_progress(AnalysisProgress {
                    stage: "Filtering names & places".to_string(),
                    progress: ner_progress.min(80),
                    detail: Some(format!("{}/{} sentences, {} names found", processed, total, found)),
                    sample_words: if samples.is_empty() { None } else { Some(samples) },
                });
            })
        } else {
            eprintln!("No proper noun candidates need NER verification");
            on_progress(AnalysisProgress {
                stage: "Filtering names & places".to_string(),
                progress: 80,
                detail: Some("No NER needed".to_string()),
                sample_words: None,
            });
            HashSet::new()
        };

        eprintln!("Found {} named entities to filter", named_entities.len());

        // Track filtered words
        let mut filtered_by_ner: Vec<String> = Vec::new();

        // Final filtering and scoring
        let mut scored_words: Vec<HardWord> = candidates
            .into_iter()
            .filter_map(|(stemmed, count, contexts, needs_ner, original_forms, _)| {
                // If it was flagged as needing NER and any form is a named entity, skip it
                if needs_ner {
                    if named_entities.contains(&stemmed) {
                        filtered_by_ner.push(stemmed.clone());
                        return None;
                    }
                    for original in &original_forms {
                        if named_entities.contains(original) {
                            filtered_by_ner.push(original.clone());
                            return None;
                        }
                    }
                }

                // Pick the best original form for display:
                // 1. Prefer forms that exist in wordfreq dictionary
                // 2. Among those, prefer the shortest (likely base form)
                // 3. Fall back to shortest original form
                let mut best_form: Option<(String, f32)> = None;
                for form in &original_forms {
                    let freq = self.wordfreq.word_frequency(form);
                    if freq > 0.0 {
                        if best_form.is_none() || form.len() < best_form.as_ref().unwrap().0.len() {
                            best_form = Some((form.clone(), freq));
                        }
                    }
                }
                let (display_word, freq) = best_form.unwrap_or_else(|| {
                    // No form in dictionary, pick shortest
                    let shortest = original_forms.iter()
                        .min_by_key(|s| s.len())
                        .cloned()
                        .unwrap_or(stemmed.clone());
                    let freq = self.wordfreq.word_frequency(&stemmed);
                    (shortest, freq)
                });

                // Clean up contexts: remove &nbsp; and highlight the word
                let clean_contexts: Vec<String> = contexts.iter()
                    .map(|ctx| {
                        ctx.replace("&nbsp;", " ")
                           .replace('\u{00A0}', " ") // non-breaking space
                           .split_whitespace()
                           .collect::<Vec<_>>()
                           .join(" ")
                    })
                    .collect();

                // Collect variants (other forms found)
                let mut variants: Vec<String> = original_forms.into_iter()
                    .filter(|f| f != &display_word)
                    .collect();
                variants.sort();

                Some(HardWord {
                    word: display_word,
                    frequency_score: freq as f64,
                    contexts: clean_contexts,
                    count,
                    variants,
                })
            })
            .collect();

        // Sort by frequency (ascending = rarest first)
        scored_words.sort_by(|a, b| {
            a.frequency_score
                .partial_cmp(&b.frequency_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        on_progress(AnalysisProgress {
            stage: "Complete".to_string(),
            progress: 100,
            detail: Some(format!("{} hard words found", scored_words.len())),
            sample_words: None,
        });

        eprintln!("Final result: {} hard words, {} filtered by NER", scored_words.len(), filtered_by_ner.len());

        let stats = AnalysisStats {
            total_candidates,
            filtered_by_ner,
            hard_words_count: scored_words.len(),
        };

        (scored_words, stats)
    }

    /// Analyze text with cancellation support
    /// Returns None if cancelled, Some((words, stats)) otherwise
    pub fn analyze_with_cancel<F>(
        &self,
        text: &str,
        frequency_threshold: f32,
        cancel_token: &Arc<AtomicBool>,
        mut on_progress: F,
    ) -> Option<(Vec<HardWord>, AnalysisStats)>
    where
        F: FnMut(AnalysisProgress),
    {
        // Check cancellation at key points
        macro_rules! check_cancel {
            () => {
                if cancel_token.load(Ordering::SeqCst) {
                    eprintln!("Analysis cancelled");
                    return None;
                }
            };
        }

        let sentences: Vec<&str> = text
            .split(|c| c == '.' || c == '!' || c == '?')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        check_cancel!();

        on_progress(AnalysisProgress {
            stage: "Analyzing text".to_string(),
            progress: 20,
            detail: Some(format!("{} sentences", sentences.len())),
            sample_words: None,
        });

        eprintln!("Processing {} sentences...", sentences.len());

        let mut word_data: HashMap<String, (usize, Vec<String>, bool, HashSet<String>, HashSet<String>)> = HashMap::new();

        for (i, sentence) in sentences.iter().enumerate() {
            // Check cancellation every 100 sentences
            if i % 100 == 0 {
                check_cancel!();
            }

            let words: Vec<&str> = sentence.unicode_words().collect();
            for word in &words {
                if word.len() < 3 || word.chars().any(|c| c.is_numeric()) {
                    continue;
                }
                let lower = word.to_lowercase();
                let stemmed = self.stem(&lower);
                let is_proper = is_likely_proper_noun(word, sentence);

                let entry = word_data.entry(stemmed.clone()).or_insert_with(|| {
                    (0, Vec::new(), false, HashSet::new(), HashSet::new())
                });
                entry.0 += 1;
                if is_proper {
                    entry.2 = true;
                }
                entry.3.insert(lower);
                let context = sentence.to_string();
                if entry.1.len() < 10 {
                    entry.1.push(context.clone());
                }
                if is_proper {
                    entry.4.insert(context);
                }
            }
        }

        check_cancel!();

        // Filter candidates using wordfreq
        let candidates: Vec<(String, usize, Vec<String>, bool, HashSet<String>, HashSet<String>)> = word_data
            .into_iter()
            .filter_map(|(stemmed, (count, contexts, needs_ner, original_forms, ner_contexts))| {
                for form in &original_forms {
                    if self.is_malformed_word(form) {
                        return None;
                    }
                }

                let mut freq = self.wordfreq.word_frequency(&stemmed);
                if freq == 0.0 {
                    for original in &original_forms {
                        let orig_freq = self.wordfreq.word_frequency(original);
                        if orig_freq > freq {
                            freq = orig_freq;
                        }
                    }
                }

                if freq > frequency_threshold || freq == 0.0 {
                    return None;
                }

                Some((stemmed, count, contexts, needs_ner, original_forms, ner_contexts))
            })
            .collect();

        check_cancel!();

        let total_candidates = candidates.len();

        on_progress(AnalysisProgress {
            stage: "Filtering names & places".to_string(),
            progress: 40,
            detail: Some(format!("{} candidates to check", total_candidates)),
            sample_words: None,
        });

        // NER filtering with progress updates
        let proper_noun_candidates: Vec<&(String, usize, Vec<String>, bool, HashSet<String>, HashSet<String>)> =
            candidates.iter().filter(|(_, _, _, needs_ner, _, _)| *needs_ner).collect();

        // Collect all candidate words that need NER checking (for display)
        let candidate_words: Vec<String> = proper_noun_candidates
            .iter()
            .flat_map(|(_, _, _, _, forms, _)| forms.iter().cloned())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        check_cancel!();

        // HARD FAIL: Resources must be available before analysis
        // Check SymSpell (required for malformed word detection)
        if !resources::is_symspell_available() {
            eprintln!("ERROR: SymSpell dictionary required but not available. Download resources first.");
            return None;
        }

        // If there are proper noun candidates, we MUST have GLiNER available
        // Fail hard if model is missing - don't silently skip NER
        if !proper_noun_candidates.is_empty() && !Self::is_gliner_available() {
            eprintln!("ERROR: GLiNER model required but not available. Download resources first.");
            return None;
        }

        let named_entities = if !proper_noun_candidates.is_empty() {
            let sentences_to_check: Vec<&str> = proper_noun_candidates
                .iter()
                .flat_map(|(_, _, _, _, _, ner_contexts)| ner_contexts.iter().map(|s| s.as_str()))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            let _total_ner_sentences = sentences_to_check.len();

            // Show candidate words before loading model
            let all_candidates: Vec<SampleWord> = candidate_words
                .iter()
                .map(|w| SampleWord {
                    word: w.clone(),
                    is_entity: false, // Not yet classified
                })
                .collect();

            on_progress(AnalysisProgress {
                stage: "Loading NER model".to_string(),
                progress: 42,
                detail: Some(format!("{} words to check", candidate_words.len())),
                sample_words: Some(all_candidates.clone()),
            });

            let mut entities = HashSet::new();
            if let Some(gliner) = self.get_gliner() {
                // Emit progress to confirm model is loaded
                on_progress(AnalysisProgress {
                    stage: "Filtering names & places".to_string(),
                    progress: 44,
                    detail: Some("NER model ready, processing...".to_string()),
                    sample_words: Some(all_candidates),
                });

                let chunks: Vec<&str> = sentences_to_check.iter()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty() && s.len() < 512)
                    .collect();

                let total_chunks = chunks.len();
                let batch_size = 64;
                let mut processed = 0;
                let mut total_infer_ms: u128 = 0;

                for (batch_idx, batch) in chunks.chunks(batch_size).enumerate() {
                    check_cancel!();

                    let pre_progress = 45 + (processed * 35 / total_chunks.max(1)) as u8;
                    on_progress(AnalysisProgress {
                        stage: "Filtering names & places".to_string(),
                        progress: pre_progress.min(79),
                        detail: Some(format!("Processing batch {}/{}...", batch_idx + 1, (total_chunks + batch_size - 1) / batch_size)),
                        sample_words: None,
                    });

                    let input = match TextInput::from_str(
                        batch,
                        &["person", "location", "organization", "country", "city"],
                    ) {
                        Ok(input) => input,
                        Err(_) => {
                            processed += batch.len();
                            continue;
                        }
                    };

                    let infer_start = std::time::Instant::now();
                    if let Ok(output) = gliner.inference(input) {
                        for spans in output.spans.iter() {
                            for span in spans.iter() {
                                let entity_text = span.text().to_lowercase();
                                entities.insert(entity_text.clone());
                                for word in entity_text.split_whitespace() {
                                    entities.insert(word.to_string());
                                }
                            }
                        }
                    }
                    let infer_elapsed = infer_start.elapsed();
                    total_infer_ms += infer_elapsed.as_millis();
                    if batch_idx == 0 {
                        eprintln!(
                            "GLiNER first batch inference: {} ms for {} sentences (batch size {})",
                            infer_elapsed.as_millis(),
                            batch.len(),
                            batch_size
                        );
                    }

                    processed += batch.len();

                    // Update progress (45% to 80% during NER inference)
                    let ner_progress = 45 + (processed * 35 / total_chunks.max(1)) as u8;

                    // Show current classification state of ALL candidate words
                    let word_states: Vec<SampleWord> = candidate_words
                        .iter()
                        .map(|w| SampleWord {
                            word: w.clone(),
                            is_entity: entities.contains(w),
                        })
                        .collect();

                    on_progress(AnalysisProgress {
                        stage: "Filtering names & places".to_string(),
                        progress: ner_progress.min(80),
                        detail: Some(format!("{}/{} sentences, {} names found", processed, total_chunks, entities.len())),
                        sample_words: Some(word_states),
                    });
                }

                if total_chunks > 0 {
                    let avg_ms = total_infer_ms as f64 / total_chunks as f64;
                    eprintln!(
                        "GLiNER total inference time: {} ms for {} sentences (avg {:.2} ms/sentence)",
                        total_infer_ms,
                        total_chunks,
                        avg_ms
                    );
                }
            }
            entities
        } else {
            // No proper noun candidates to check - skip NER entirely
            on_progress(AnalysisProgress {
                stage: "Filtering names & places".to_string(),
                progress: 80,
                detail: Some("No proper noun candidates".to_string()),
                sample_words: None,
            });
            HashSet::new()
        };

        check_cancel!();

        let mut filtered_by_ner: Vec<String> = Vec::new();

        let mut scored_words: Vec<HardWord> = candidates
            .into_iter()
            .filter_map(|(stemmed, count, contexts, needs_ner, original_forms, _)| {
                if needs_ner {
                    if named_entities.contains(&stemmed) {
                        filtered_by_ner.push(stemmed.clone());
                        return None;
                    }
                    for original in &original_forms {
                        if named_entities.contains(original) {
                            filtered_by_ner.push(original.clone());
                            return None;
                        }
                    }
                }

                let mut best_form: Option<(String, f32)> = None;
                for form in &original_forms {
                    let freq = self.wordfreq.word_frequency(form);
                    if freq > 0.0 {
                        if best_form.is_none() || form.len() < best_form.as_ref().unwrap().0.len() {
                            best_form = Some((form.clone(), freq));
                        }
                    }
                }
                let (display_word, freq) = best_form.unwrap_or_else(|| {
                    let shortest = original_forms.iter()
                        .min_by_key(|s| s.len())
                        .cloned()
                        .unwrap_or(stemmed.clone());
                    let freq = self.wordfreq.word_frequency(&stemmed);
                    (shortest, freq)
                });

                let clean_contexts: Vec<String> = contexts.iter()
                    .map(|ctx| {
                        ctx.replace("&nbsp;", " ")
                           .replace('\u{00A0}', " ")
                           .split_whitespace()
                           .collect::<Vec<_>>()
                           .join(" ")
                    })
                    .collect();

                let mut variants: Vec<String> = original_forms.into_iter()
                    .filter(|f| f != &display_word)
                    .collect();
                variants.sort();

                Some(HardWord {
                    word: display_word,
                    frequency_score: freq as f64,
                    contexts: clean_contexts,
                    count,
                    variants,
                })
            })
            .collect();

        scored_words.sort_by(|a, b| {
            a.frequency_score.partial_cmp(&b.frequency_score).unwrap_or(std::cmp::Ordering::Equal)
        });

        on_progress(AnalysisProgress {
            stage: "Complete".to_string(),
            progress: 100,
            detail: Some(format!("{} hard words found", scored_words.len())),
            sample_words: None,
        });

        let stats = AnalysisStats {
            total_candidates,
            filtered_by_ner,
            hard_words_count: scored_words.len(),
        };

        Some((scored_words, stats))
    }
}

fn get_symspell() -> Option<&'static SymSpell<AsciiStringStrategy>> {
    SYMSPELL.get_or_init(|| {
        // Use the resource system to ensure dictionary is available
        let dict_path = match resources::ensure_symspell_dict(|_status| {
            // Silent download for symspell (it's small)
        }) {
            Ok(path) => path,
            Err(e) => {
                eprintln!("Failed to get SymSpell dictionary: {}", e);
                return None;
            }
        };

        let mut symspell: SymSpell<AsciiStringStrategy> = SymSpell::default();

        let loaded = symspell.load_dictionary(
            dict_path.to_str().unwrap_or(""),
            0,
            1,
            " ",
        );

        if !loaded {
            eprintln!("Failed to load SymSpell dictionary from {:?}", dict_path);
            return None;
        }

        eprintln!("SymSpell dictionary loaded successfully");
        Some(symspell)
    }).as_ref()
}

fn is_likely_proper_noun(word: &str, sentence: &str) -> bool {
    let first_char = word.chars().next();
    if let Some(c) = first_char {
        if !c.is_uppercase() {
            return false;
        }

        // Check if it's at the start of the sentence
        let trimmed = sentence.trim_start();
        if trimmed.starts_with(word) {
            return false;
        }

        // Capitalized in the middle of a sentence = likely proper noun
        true
    } else {
        false
    }
}

impl Default for NlpPipeline {
    fn default() -> Self {
        Self::new()
    }
}

use gliner::model::{GLiNER, input::text::TextInput, pipeline::span::SpanMode};
use orp::params::RuntimeParameters;

#[cfg(target_os = "macos")]
use ort::execution_providers::CoreMLExecutionProvider;
use rust_stemmers::{Algorithm, Stemmer};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;
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
}

#[derive(Debug, Serialize, Clone)]
pub struct AnalysisStats {
    pub total_candidates: usize,
    pub filtered_by_ner: Vec<String>,
    pub hard_words_count: usize,
}

static GLINER_MODEL: OnceLock<Option<GLiNER<SpanMode>>> = OnceLock::new();

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

    pub fn is_gliner_available() -> bool {
        let model_dir = get_gliner_model_dir();
        let tokenizer_path = model_dir.join("tokenizer.json");
        let model_path = model_dir.join("model.onnx");
        tokenizer_path.exists() && model_path.exists()
    }

    fn get_gliner(&self) -> Option<&GLiNER<SpanMode>> {
        GLINER_MODEL.get_or_init(|| {
            let model_dir = get_gliner_model_dir();
            let tokenizer_path = model_dir.join("tokenizer.json");
            let model_path = model_dir.join("model.onnx");

            if !tokenizer_path.exists() || !model_path.exists() {
                eprintln!("GLiNER model not found at {:?}", model_dir);
                return None;
            }

            // Configure runtime with CoreML on macOS for better performance
            // Use more threads for better parallelism
            #[cfg(target_os = "macos")]
            let runtime_params = RuntimeParameters::default()
                .with_threads(8)
                .with_execution_providers([CoreMLExecutionProvider::default().build()]);

            #[cfg(not(target_os = "macos"))]
            let runtime_params = RuntimeParameters::default().with_threads(8);

            match GLiNER::<SpanMode>::new(
                Default::default(),
                runtime_params,
                tokenizer_path,
                model_path,
            ) {
                Ok(model) => {
                    eprintln!("GLiNER model loaded successfully (CoreML enabled on macOS)");
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
        F: FnMut(usize, usize, usize), // (sentences_processed, total_sentences, entities_found)
    {
        let mut entities = HashSet::new();

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
        let batch_size = 16;
        let mut processed = 0;

        for batch in chunks.chunks(batch_size) {
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

            match gliner.inference(input) {
                Ok(output) => {
                    for spans in output.spans.iter() {
                        for span in spans.iter() {
                            let entity_text = span.text().to_lowercase();
                            entities.insert(entity_text.clone());
                            // Also add individual words from multi-word entities
                            for word in entity_text.split_whitespace() {
                                entities.insert(word.to_string());
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("GLiNER inference error: {}", e);
                }
            }

            processed += batch.len();
            // Report progress after processing each batch
            on_progress(processed, total_sentences, entities.len());
        }

        eprintln!("GLiNER found {} unique entities", entities.len());
        entities
    }

    pub fn analyze<F>(&self, text: &str, mut on_progress: F) -> (Vec<HardWord>, AnalysisStats)
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
        });

        eprintln!("Processing {} sentences...", sentences.len());

        // FIRST PASS: Collect word counts and identify hard word CANDIDATES using wordfreq
        // This is fast and filters out most words before we even touch GLiNER
        // Key is stemmed form, value is (count, contexts, is_proper_noun_candidate, original_forms)
        let mut word_data: HashMap<String, (usize, Vec<String>, bool, HashSet<String>)> = HashMap::new();

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

                let entry = word_data.entry(stemmed).or_insert((0, Vec::new(), false, HashSet::new()));
                entry.0 += 1;
                if is_proper {
                    entry.2 = true; // Mark as needing NER check
                }
                entry.3.insert(lower); // Track original forms

                // Store context sentence (limit to 3 per word)
                if entry.1.len() < 3 && sentence.len() > 20 && sentence.len() < 500 {
                    let context = format!("{}.", sentence);
                    if !entry.1.contains(&context) {
                        entry.1.push(context);
                    }
                }
            }
        }

        // Filter to get hard word candidates based on frequency
        // Use stemmed form for frequency lookup, but try original forms too
        let candidates: Vec<(String, usize, Vec<String>, bool, HashSet<String>)> = word_data
            .into_iter()
            .filter_map(|(stemmed, (count, contexts, needs_ner, original_forms))| {
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
                if freq > 0.0001 || freq == 0.0 {
                    return None;
                }

                Some((stemmed, count, contexts, needs_ner, original_forms))
            })
            .collect();

        eprintln!("Found {} hard word candidates after wordfreq filtering", candidates.len());

        // SECOND PASS: Only run GLiNER on sentences containing candidates that need NER verification
        // This is MUCH faster than running on the entire book
        let sentences_needing_ner: Vec<&str> = candidates
            .iter()
            .filter(|(_, _, _, needs_ner, _)| *needs_ner)
            .flat_map(|(_, _, contexts, _, _)| {
                contexts.iter().map(|c| c.trim_end_matches('.'))
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        let total_candidates = candidates.len();
        let named_entities = if !sentences_needing_ner.is_empty() {
            let total_ner_sentences = sentences_needing_ner.len();
            eprintln!("Running NER on {} sentences containing proper noun candidates...", total_ner_sentences);

            on_progress(AnalysisProgress {
                stage: "Filtering names & places".to_string(),
                progress: 40,
                detail: Some(format!("0/{} sentences", total_ner_sentences)),
            });

            self.extract_entities_from_sentences(&sentences_needing_ner, |processed, total, found| {
                let ner_progress = 40 + (processed * 40 / total.max(1)) as u8;
                on_progress(AnalysisProgress {
                    stage: "Filtering names & places".to_string(),
                    progress: ner_progress.min(80),
                    detail: Some(format!("{}/{} sentences, {} names found", processed, total, found)),
                });
            })
        } else {
            eprintln!("No proper noun candidates need NER verification");
            on_progress(AnalysisProgress {
                stage: "Filtering names & places".to_string(),
                progress: 80,
                detail: Some("No NER needed".to_string()),
            });
            HashSet::new()
        };

        eprintln!("Found {} named entities to filter", named_entities.len());

        // Track filtered words
        let mut filtered_by_ner: Vec<String> = Vec::new();

        // Final filtering and scoring
        let mut scored_words: Vec<HardWord> = candidates
            .into_iter()
            .filter_map(|(stemmed, count, contexts, needs_ner, original_forms)| {
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
        });

        eprintln!("Final result: {} hard words, {} filtered by NER", scored_words.len(), filtered_by_ner.len());

        let stats = AnalysisStats {
            total_candidates,
            filtered_by_ner,
            hard_words_count: scored_words.len(),
        };

        (scored_words, stats)
    }
}

fn get_gliner_model_dir() -> std::path::PathBuf {
    // Check for bundled resources first
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let bundled = exe_dir.join("resources").join("gliner");
            if bundled.exists() {
                return bundled;
            }
            // macOS app bundle
            let macos_bundled = exe_dir.join("../Resources/gliner");
            if macos_bundled.exists() {
                return macos_bundled;
            }
        }
    }

    // Development path
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join("gliner");
    if dev_path.exists() {
        return dev_path;
    }

    // User data directory fallback
    dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("lexis")
        .join("models")
        .join("gliner")
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

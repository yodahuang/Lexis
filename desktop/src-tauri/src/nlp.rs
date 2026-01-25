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
    fn extract_entities_from_sentences(&self, sentences: &[&str]) -> HashSet<String> {
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

        eprintln!("Running GLiNER on {} sentences...", chunks.len());

        // Process in smaller batches for better CoreML utilization
        let batch_size = 32;
        for (batch_idx, batch) in chunks.chunks(batch_size).enumerate() {
            let input = match TextInput::from_str(
                batch,
                &["person", "location", "organization", "country", "city"],
            ) {
                Ok(input) => input,
                Err(e) => {
                    eprintln!("Failed to create GLiNER input for batch {}: {}", batch_idx, e);
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
                    eprintln!("GLiNER inference error on batch {}: {}", batch_idx, e);
                }
            }
        }

        eprintln!("GLiNER found {} unique entities", entities.len());
        entities
    }

    pub fn analyze(&self, text: &str) -> Vec<HardWord> {
        // Split into sentences for context
        let sentences: Vec<&str> = text
            .split(|c| c == '.' || c == '!' || c == '?')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

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
            .take(100) // Limit to 100 unique sentences for NER
            .collect();

        let named_entities = if !sentences_needing_ner.is_empty() {
            eprintln!("Running NER on {} sentences containing proper noun candidates...", sentences_needing_ner.len());
            self.extract_entities_from_sentences(&sentences_needing_ner)
        } else {
            eprintln!("No proper noun candidates need NER verification");
            HashSet::new()
        };

        eprintln!("Found {} named entities to filter", named_entities.len());

        // Final filtering and scoring
        let mut scored_words: Vec<HardWord> = candidates
            .into_iter()
            .filter_map(|(stemmed, count, contexts, needs_ner, original_forms)| {
                // If it was flagged as needing NER and any form is a named entity, skip it
                if needs_ner {
                    if named_entities.contains(&stemmed) {
                        return None;
                    }
                    for original in &original_forms {
                        if named_entities.contains(original) {
                            return None;
                        }
                    }
                }

                // Pick the most common original form for display
                let display_word = original_forms.into_iter().next().unwrap_or(stemmed.clone());

                // Get frequency for scoring
                let mut freq = self.wordfreq.word_frequency(&stemmed);
                if freq == 0.0 {
                    freq = self.wordfreq.word_frequency(&display_word);
                }

                Some(HardWord {
                    word: display_word,
                    frequency_score: freq as f64,
                    contexts,
                    count,
                })
            })
            .collect();

        // Sort by frequency (ascending = rarest first)
        scored_words.sort_by(|a, b| {
            a.frequency_score
                .partial_cmp(&b.frequency_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        eprintln!("Final result: {} hard words", scored_words.len());
        scored_words
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

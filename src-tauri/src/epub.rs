use ammonia::Builder;
use epub::doc::EpubDoc;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum EpubError {
    #[error("Failed to open EPUB: {0}")]
    Open(String),
    #[error("Failed to read chapter: {0}")]
    ReadChapter(String),
}

impl serde::Serialize for EpubError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub struct ExtractedText {
    pub full_text: String,
    pub chapter_count: usize,
}

pub fn extract_text(epub_path: &Path) -> Result<ExtractedText, EpubError> {
    let mut doc = EpubDoc::new(epub_path).map_err(|e| EpubError::Open(e.to_string()))?;

    let mut full_text = String::new();
    let mut chapter_count = 0;

    // Build HTML cleaner - strip all tags, keep only text
    let mut cleaner = Builder::new();
    cleaner
        .tags(HashSet::new()) // No tags allowed - strips everything
        .clean_content_tags(HashSet::from(["script", "style"]));

    // Iterate through spine (reading order)
    while doc.go_next() {
        if let Some((content, _mime)) = doc.get_current_str() {
            // Clean HTML to plain text
            let clean = cleaner.clean(&content).to_string();

            // Normalize whitespace
            let normalized: String = clean
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");

            if !normalized.is_empty() {
                if !full_text.is_empty() {
                    full_text.push_str("\n\n");
                }
                full_text.push_str(&normalized);
                chapter_count += 1;
            }
        }
    }

    Ok(ExtractedText {
        full_text,
        chapter_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_cleaning() {
        let mut cleaner = Builder::new();
        cleaner
            .tags(HashSet::new())
            .clean_content_tags(HashSet::from(["script", "style"]));

        let html = r#"<html><body><h1>Title</h1><p>Hello <b>world</b>!</p><script>evil()</script></body></html>"#;
        let clean = cleaner.clean(html).to_string();
        let normalized: String = clean.split_whitespace().collect::<Vec<_>>().join(" ");

        assert_eq!(normalized, "Title Hello world !");
    }
}

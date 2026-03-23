//! Plain text and Markdown extractors.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extraction::text::parse_text;
use crate::plugins::{DocumentExtractor, Plugin};
use crate::types::ExtractionResult;
use async_trait::async_trait;

/// Plain text extractor.
///
/// Extracts content from plain text files (.txt).
pub struct PlainTextExtractor;

impl PlainTextExtractor {
    /// Create a new plain text extractor.
    pub fn new() -> Self {
        Self
    }
}

impl Default for PlainTextExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for PlainTextExtractor {
    fn name(&self) -> &str {
        "plain-text-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    fn description(&self) -> &str {
        "Extracts content from plain text files"
    }

    fn author(&self) -> &str {
        "Kreuzberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl DocumentExtractor for PlainTextExtractor {
    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self, content, config),
        fields(
            extractor.name = self.name(),
            content.size_bytes = content.len(),
        )
    ))]
    async fn extract_bytes(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<ExtractionResult> {
        let text = String::from_utf8_lossy(content).into_owned();
        let text = text.trim_end_matches('\n').trim_end_matches('\r').to_string();
        let line_count = text.lines().count();
        let word_count = text.split_whitespace().count();
        let character_count = text.len();

        let document = if config.include_document_structure {
            use crate::types::builder::DocumentStructureBuilder;
            let mut builder = DocumentStructureBuilder::new().source_format("text");
            for paragraph in text.split("\n\n") {
                let trimmed = paragraph.trim();
                if !trimmed.is_empty() {
                    builder.push_paragraph(trimmed, vec![], None, None);
                }
            }
            Some(builder.build())
        } else {
            None
        };

        Ok(ExtractionResult {
            content: text,
            mime_type: mime_type.to_string().into(),
            metadata: crate::types::Metadata {
                format: Some(crate::types::FormatMetadata::Text(crate::types::TextMetadata {
                    line_count,
                    word_count,
                    character_count,
                    headers: None,
                    links: None,
                    code_blocks: None,
                })),
                ..Default::default()
            },
            pages: None,
            tables: vec![],
            detected_languages: None,
            chunks: None,
            images: None,
            elements: None,
            djot_content: None,
            ocr_elements: None,
            document,
            #[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
            extracted_keywords: None,
            quality_score: None,
            processing_warnings: Vec::new(),
            annotations: None,
        })
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[
            "text/plain",
            "text/troff",
            "text/x-mdoc",
            "text/x-pod",
            "text/x-dokuwiki",
        ]
    }

    fn priority(&self) -> i32 {
        50
    }
}

/// Markdown extractor.
///
/// Extracts content from Markdown files (.md, .markdown).
/// Preserves markdown syntax and extracts metadata like headers, links, and code blocks.
pub struct MarkdownExtractor;

impl MarkdownExtractor {
    /// Create a new Markdown extractor.
    pub fn new() -> Self {
        Self
    }
}

impl Default for MarkdownExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for MarkdownExtractor {
    fn name(&self) -> &str {
        "markdown-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    fn description(&self) -> &str {
        "Extracts content from Markdown files with metadata parsing"
    }

    fn author(&self) -> &str {
        "Kreuzberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl DocumentExtractor for MarkdownExtractor {
    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self, content, config),
        fields(
            extractor.name = self.name(),
            content.size_bytes = content.len(),
        )
    ))]
    async fn extract_bytes(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<ExtractionResult> {
        let text_result = parse_text(content, true)?;

        let document = if config.include_document_structure {
            Some(build_markdown_document_structure(&text_result.content))
        } else {
            None
        };

        Ok(ExtractionResult {
            content: text_result.content,
            mime_type: mime_type.to_string().into(),
            metadata: crate::types::Metadata {
                format: Some(crate::types::FormatMetadata::Text(crate::types::TextMetadata {
                    line_count: text_result.line_count,
                    word_count: text_result.word_count,
                    character_count: text_result.character_count,
                    headers: text_result.headers,
                    links: text_result.links,
                    code_blocks: text_result.code_blocks,
                })),
                ..Default::default()
            },
            pages: None,
            tables: vec![],
            detected_languages: None,
            chunks: None,
            images: None,
            elements: None,
            djot_content: None,
            ocr_elements: None,
            document,
            #[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
            extracted_keywords: None,
            quality_score: None,
            processing_warnings: Vec::new(),
            annotations: None,
        })
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[
            "text/markdown",
            "text/x-markdown",
            "text/x-markdown-extra",
            "text/x-multimarkdown",
        ]
    }

    fn priority(&self) -> i32 {
        50
    }
}

/// Build a `DocumentStructure` from markdown content using line-by-line parsing.
///
/// Recognizes:
/// - Headings (`#`, `##`, `###`, etc.)
/// - Fenced code blocks (`` ``` ``)
/// - Unordered lists (`- ` or `* `)
/// - Ordered lists (`1. `, `2. `, etc.)
/// - Paragraphs (text separated by blank lines)
fn build_markdown_document_structure(content: &str) -> crate::types::document_structure::DocumentStructure {
    use crate::types::builder::DocumentStructureBuilder;

    let mut builder = DocumentStructureBuilder::new().source_format("markdown");

    let lines: Vec<&str> = content.lines().collect();
    let len = lines.len();
    let mut i = 0;

    while i < len {
        let line = lines[i];

        // Fenced code block
        if line.starts_with("```") {
            let lang = line.trim_start_matches('`').trim();
            let lang_opt = if lang.is_empty() { None } else { Some(lang) };
            let mut code_lines = Vec::new();
            i += 1;
            while i < len && !lines[i].starts_with("```") {
                code_lines.push(lines[i]);
                i += 1;
            }
            // Skip closing ```
            if i < len {
                i += 1;
            }
            let code_text = code_lines.join("\n");
            builder.push_code(&code_text, lang_opt, None);
            continue;
        }

        // Heading
        if line.starts_with('#') {
            let level = line.bytes().take_while(|&b| b == b'#').count() as u8;
            if level <= 6 {
                let text = line[level as usize..].trim_start_matches(' ');
                if !text.is_empty() {
                    builder.push_heading(level, text, None, None);
                    i += 1;
                    continue;
                }
            }
        }

        // Unordered list (- or *)
        if line.starts_with("- ") || line.starts_with("* ") {
            let list_idx = builder.push_list(false, None);
            while i < len && (lines[i].starts_with("- ") || lines[i].starts_with("* ")) {
                let item_text = lines[i][2..].trim();
                builder.push_list_item(list_idx, item_text, None);
                i += 1;
            }
            continue;
        }

        // Ordered list (digits followed by `. `)
        if is_ordered_list_item(line) {
            let list_idx = builder.push_list(true, None);
            while i < len && is_ordered_list_item(lines[i]) {
                let item_text = lines[i].split_once(". ").map(|x| x.1).unwrap_or("").trim();
                builder.push_list_item(list_idx, item_text, None);
                i += 1;
            }
            continue;
        }

        // Blank line — skip
        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        // Paragraph: collect consecutive non-blank, non-structural lines
        let mut para_lines = Vec::new();
        while i < len {
            let l = lines[i];
            if l.trim().is_empty()
                || l.starts_with('#')
                || l.starts_with("```")
                || l.starts_with("- ")
                || l.starts_with("* ")
                || is_ordered_list_item(l)
            {
                break;
            }
            para_lines.push(l);
            i += 1;
        }
        if !para_lines.is_empty() {
            let para_text = para_lines.join("\n");
            builder.push_paragraph(para_text.trim(), vec![], None, None);
        }
    }

    builder.build()
}

/// Check if a line is an ordered list item (e.g. `1. `, `23. `).
fn is_ordered_list_item(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut idx = 0;
    // Must start with at least one digit
    if idx >= bytes.len() || !bytes[idx].is_ascii_digit() {
        return false;
    }
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    // Must be followed by `. `
    idx + 1 < bytes.len() && bytes[idx] == b'.' && bytes[idx + 1] == b' '
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_plain_text_extractor() {
        let extractor = PlainTextExtractor::new();
        let content = b"Hello, World!\nThis is a test.";
        let config = ExtractionConfig::default();

        let result = extractor.extract_bytes(content, "text/plain", &config).await.unwrap();

        assert_eq!(result.mime_type, "text/plain");
        assert!(result.content.contains("Hello, World!"));
        assert!(result.metadata.format.is_some());
        let text_meta = match result.metadata.format.as_ref().unwrap() {
            crate::types::FormatMetadata::Text(meta) => meta,
            _ => panic!("Expected Text metadata"),
        };
        assert_eq!(text_meta.line_count, 2);
        assert_eq!(text_meta.word_count, 6);
    }

    #[tokio::test]
    async fn test_markdown_extractor() {
        let extractor = MarkdownExtractor::new();
        let content = b"# Header\n\nThis is [a link](https://example.com).\n\n```python\nprint(\"hello\")\n```";
        let config = ExtractionConfig::default();

        let result = extractor
            .extract_bytes(content, "text/markdown", &config)
            .await
            .unwrap();

        assert_eq!(result.mime_type, "text/markdown");
        assert!(result.content.contains("# Header"));
        assert!(result.metadata.format.is_some());
        let text_meta = match result.metadata.format.as_ref().unwrap() {
            crate::types::FormatMetadata::Text(meta) => meta,
            _ => panic!("Expected Text metadata"),
        };
        assert!(text_meta.headers.is_some());
        assert!(text_meta.links.is_some());
        assert!(text_meta.code_blocks.is_some());
    }

    #[test]
    fn test_plain_text_plugin_interface() {
        let extractor = PlainTextExtractor::new();
        assert_eq!(extractor.name(), "plain-text-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(
            extractor.supported_mime_types(),
            &[
                "text/plain",
                "text/troff",
                "text/x-mdoc",
                "text/x-pod",
                "text/x-dokuwiki",
            ]
        );
        assert_eq!(extractor.priority(), 50);
    }

    #[test]
    fn test_markdown_plugin_interface() {
        let extractor = MarkdownExtractor::new();
        assert_eq!(extractor.name(), "markdown-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(
            extractor.supported_mime_types(),
            &[
                "text/markdown",
                "text/x-markdown",
                "text/x-markdown-extra",
                "text/x-multimarkdown"
            ]
        );
        assert_eq!(extractor.priority(), 50);
    }
}

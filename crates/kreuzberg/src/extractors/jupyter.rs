//! Jupyter Notebook extractor for .ipynb files.
//!
//! This extractor provides native Rust parsing of Jupyter notebooks,
//! extracting:
//! - Notebook metadata (kernelspec, language_info, nbformat)
//! - Cell content (markdown and code cells in order)
//! - Cell outputs (text, HTML, images)
//! - Cell metadata (execution_count, tags)
//!
//! Requires the `office` feature.

#[cfg(feature = "office")]
use crate::Result;
#[cfg(feature = "office")]
use crate::core::config::ExtractionConfig;
#[cfg(feature = "office")]
use crate::plugins::{DocumentExtractor, Plugin};
#[cfg(feature = "office")]
use crate::types::builder::DocumentStructureBuilder;
#[cfg(feature = "office")]
use crate::types::{ExtractedImage, ExtractionResult, Metadata};
#[cfg(feature = "office")]
use ahash::AHashMap;
#[cfg(feature = "office")]
use async_trait::async_trait;
#[cfg(feature = "office")]
use base64::Engine;
#[cfg(feature = "office")]
use bytes::Bytes;
#[cfg(feature = "office")]
use serde_json::{Value, json};
#[cfg(feature = "office")]
use std::borrow::Cow;

#[cfg(feature = "office")]
type NotebookContent = (String, AHashMap<Cow<'static, str>, Value>, Vec<ExtractedImage>, Value);

/// Jupyter Notebook extractor.
///
/// Extracts content from Jupyter notebook JSON files, including:
/// - Notebook metadata (kernel, language, nbformat version)
/// - Cell content (code and markdown)
/// - Cell outputs (text, HTML, etc.)
/// - Cell-level metadata (tags, execution counts)
#[cfg(feature = "office")]
pub struct JupyterExtractor;

#[cfg(feature = "office")]
impl JupyterExtractor {
    /// Create a new Jupyter extractor.
    pub fn new() -> Self {
        Self
    }

    /// Extract content from a Jupyter notebook.
    fn extract_notebook(content: &[u8], plain: bool) -> Result<NotebookContent> {
        let notebook: Value = serde_json::from_slice(content)
            .map_err(|e| crate::KreuzbergError::parsing(format!("Failed to parse JSON: {}", e)))?;

        let mut extracted_content = String::new();
        let mut metadata = AHashMap::new();
        let mut images = Vec::new();

        if let Some(notebook_metadata) = notebook.get("metadata").and_then(|m| m.as_object()) {
            if let Some(kernelspec) = notebook_metadata.get("kernelspec") {
                metadata.insert(Cow::Borrowed("kernelspec"), kernelspec.clone());
            }

            if let Some(language_info) = notebook_metadata.get("language_info") {
                metadata.insert(Cow::Borrowed("language_info"), language_info.clone());
            }
        }

        if let Some(nbformat) = notebook.get("nbformat") {
            metadata.insert(Cow::Borrowed("nbformat"), nbformat.clone());
        }

        if let Some(cells) = notebook.get("cells").and_then(|c| c.as_array()) {
            for (cell_idx, cell) in cells.iter().enumerate() {
                Self::extract_cell(
                    cell,
                    cell_idx,
                    &mut extracted_content,
                    &mut metadata,
                    &mut images,
                    plain,
                )?;
            }
        }

        Ok((extracted_content, metadata, images, notebook))
    }

    /// Extract content from a single cell.
    fn extract_cell(
        cell: &Value,
        cell_idx: usize,
        content: &mut String,
        _metadata: &mut AHashMap<Cow<'static, str>, Value>,
        images: &mut Vec<ExtractedImage>,
        plain: bool,
    ) -> Result<()> {
        let cell_type = cell.get("cell_type").and_then(|t| t.as_str()).unwrap_or("unknown");

        match cell_type {
            "markdown" => Self::extract_markdown_cell(cell, content)?,
            "code" => Self::extract_code_cell(cell, cell_idx, content, images, plain)?,
            "raw" => Self::extract_raw_cell(cell, content)?,
            _ => {}
        }

        // Separate cells with a blank line
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push('\n');
        Ok(())
    }

    /// Extract markdown cell content.
    fn extract_markdown_cell(cell: &Value, content: &mut String) -> Result<()> {
        if let Some(source) = cell.get("source") {
            let cell_text = Self::extract_source(source);
            content.push_str(&cell_text);
        }
        Ok(())
    }

    /// Extract code cell content and outputs.
    fn extract_code_cell(
        cell: &Value,
        cell_idx: usize,
        content: &mut String,
        images: &mut Vec<ExtractedImage>,
        plain: bool,
    ) -> Result<()> {
        if let Some(source) = cell.get("source") {
            let cell_text = Self::extract_source(source);
            content.push_str(&cell_text);
            if !cell_text.ends_with('\n') {
                content.push('\n');
            }
        }

        if let Some(outputs) = cell.get("outputs").and_then(|o| o.as_array()) {
            for output in outputs {
                Self::extract_output(output, cell_idx, content, images, plain)?;
            }
        }

        Ok(())
    }

    /// Extract raw cell content.
    fn extract_raw_cell(cell: &Value, content: &mut String) -> Result<()> {
        if let Some(source) = cell.get("source") {
            let cell_text = Self::extract_source(source);
            content.push_str(&cell_text);
        }
        Ok(())
    }

    /// Extract source content from various formats.
    ///
    /// Source can be either a string or an array of strings.
    fn extract_source(source: &Value) -> String {
        match source {
            Value::String(s) => s.clone(),
            Value::Array(arr) => arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(""),
            _ => String::new(),
        }
    }

    /// Extract output from a cell.
    fn extract_output(
        output: &Value,
        cell_idx: usize,
        content: &mut String,
        images: &mut Vec<ExtractedImage>,
        plain: bool,
    ) -> Result<()> {
        let output_type = output.get("output_type").and_then(|t| t.as_str()).unwrap_or("unknown");

        match output_type {
            "stream" => Self::extract_stream_output(output, content)?,
            "execute_result" | "display_data" => {
                Self::extract_data_output(output, cell_idx, content, images, plain)?;
            }
            "error" => Self::extract_error_output(output, content)?,
            _ => {}
        }

        Ok(())
    }

    /// Extract stream output (stdout, stderr).
    fn extract_stream_output(output: &Value, content: &mut String) -> Result<()> {
        if let Some(text) = output.get("text") {
            let text_content = Self::extract_source(text);
            content.push_str(&text_content);
        }

        Ok(())
    }

    /// Extract data output (execute_result or display_data).
    ///
    /// Prioritizes text/plain for quality scoring. For raster image types,
    /// decodes base64 data and populates the images collection.
    fn extract_data_output(
        output: &Value,
        cell_idx: usize,
        content: &mut String,
        images: &mut Vec<ExtractedImage>,
        plain_mode: bool,
    ) -> Result<()> {
        if let Some(data) = output.get("data").and_then(|d| d.as_object()) {
            // Prefer text/plain first - it has the most readable tokens for quality scoring
            if let Some(plain) = data.get("text/plain") {
                let text = Self::extract_source(plain);
                if !text.is_empty() {
                    content.push_str(&text);
                    if !text.ends_with('\n') {
                        content.push('\n');
                    }
                }
            }

            // Also include markdown/HTML content — these often contain richer
            // semantic information than text/plain (e.g. descriptive fallback text).
            // Skip these for plain text output mode.
            if !plain_mode {
                for mime_type in &["text/markdown", "text/html"] {
                    if let Some(mime_content) = data.get(*mime_type) {
                        let mime_text = Self::extract_source(mime_content);
                        if !mime_text.is_empty() {
                            content.push_str(&mime_text);
                            if !mime_text.ends_with('\n') {
                                content.push('\n');
                            }
                        }
                    }
                }
            }

            // For raster image types, extract actual base64-encoded image data
            for mime_type in &["image/png", "image/jpeg", "image/gif", "image/webp"] {
                if let Some(image_value) = data.get(*mime_type) {
                    let base64_str = Self::extract_source(image_value);
                    let cleaned = base64_str.replace(['\n', '\r'], "");
                    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&cleaned) {
                        let format = match *mime_type {
                            "image/png" => "png",
                            "image/jpeg" => "jpeg",
                            "image/gif" => "gif",
                            "image/webp" => "webp",
                            _ => "unknown",
                        };
                        images.push(ExtractedImage {
                            data: Bytes::from(decoded),
                            format: Cow::Borrowed(format),
                            image_index: images.len(),
                            page_number: Some(cell_idx + 1),
                            width: None,
                            height: None,
                            colorspace: None,
                            bits_per_component: None,
                            is_mask: false,
                            description: Some(format!("Notebook cell {} output", cell_idx)),
                            ocr_result: None,
                            bounding_box: None,
                        });
                        content.push_str(&format!("[Image: {}]\n", mime_type));
                    }
                }
            }

            // Handle SVG as text (not a raster image for OCR)
            if data.contains_key("image/svg+xml") {
                content.push_str("[Image: image/svg+xml]\n");
            }

            // Include JSON output as structured data
            if let Some(json_content) = data.get("application/json")
                && let Ok(formatted) = serde_json::to_string_pretty(json_content)
            {
                content.push_str(&formatted);
                content.push('\n');
            }
        }

        Ok(())
    }

    /// Build a `DocumentStructure` from the already-parsed notebook JSON.
    ///
    /// Markdown cells become paragraphs, code cells become code blocks.
    fn build_document_structure(notebook: &Value) -> Option<crate::types::document_structure::DocumentStructure> {
        let cells = notebook.get("cells")?.as_array()?;

        // Determine the kernel language for code cells
        let kernel_lang = notebook
            .get("metadata")
            .and_then(|m| m.get("kernelspec"))
            .and_then(|k| k.get("language"))
            .and_then(|l| l.as_str())
            .or_else(|| {
                notebook
                    .get("metadata")
                    .and_then(|m| m.get("language_info"))
                    .and_then(|l| l.get("name"))
                    .and_then(|n| n.as_str())
            });

        let mut builder = DocumentStructureBuilder::new().source_format("jupyter");

        for cell in cells {
            let cell_type = cell.get("cell_type").and_then(|t| t.as_str()).unwrap_or("unknown");
            let source_text = Self::extract_source(cell.get("source").unwrap_or(&Value::Null));
            let trimmed = source_text.trim();
            if trimmed.is_empty() {
                continue;
            }

            match cell_type {
                "markdown" => {
                    builder.push_paragraph(trimmed, vec![], None, None);
                }
                "code" => {
                    builder.push_code(trimmed, kernel_lang, None);
                }
                _ => {
                    builder.push_paragraph(trimmed, vec![], None, None);
                }
            }
        }

        Some(builder.build())
    }

    /// Extract error output.
    fn extract_error_output(output: &Value, content: &mut String) -> Result<()> {
        if let Some(ename) = output.get("ename").and_then(|e| e.as_str()) {
            content.push_str(&format!("Error: {}\n", ename));
        }

        if let Some(evalue) = output.get("evalue").and_then(|e| e.as_str()) {
            content.push_str(&format!("Value: {}\n", evalue));
        }

        if let Some(traceback) = output.get("traceback").and_then(|t| t.as_array()) {
            for line in traceback {
                if let Some(line_str) = line.as_str() {
                    content.push_str(line_str);
                    content.push('\n');
                }
            }
        }

        Ok(())
    }
}

#[cfg(feature = "office")]
impl Default for JupyterExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "office")]
impl Plugin for JupyterExtractor {
    fn name(&self) -> &str {
        "jupyter-extractor"
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
        "Extracts content from Jupyter notebooks (.ipynb files)"
    }

    fn author(&self) -> &str {
        "Kreuzberg Team"
    }
}

#[cfg(feature = "office")]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl DocumentExtractor for JupyterExtractor {
    #[cfg_attr(
        feature = "otel",
        tracing::instrument(
            skip(self, content, config),
            fields(
                extractor.name = self.name(),
                content.size_bytes = content.len(),
            )
        )
    )]
    async fn extract_bytes(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<ExtractionResult> {
        let plain = matches!(
            config.output_format,
            crate::core::config::OutputFormat::Plain | crate::core::config::OutputFormat::Structured
        );
        let (extracted_content, additional_metadata, extracted_images, notebook_json) =
            Self::extract_notebook(content, plain)?;

        let mut metadata_additional = AHashMap::new();
        for (key, value) in additional_metadata {
            metadata_additional.insert(key, json!(value));
        }

        // Process images with OCR if configured and available
        let images = if !extracted_images.is_empty() {
            #[cfg(all(feature = "ocr", feature = "tokio-runtime"))]
            {
                let processed = crate::extraction::image_ocr::process_images_with_ocr(extracted_images, config).await?;
                Some(processed)
            }
            #[cfg(not(all(feature = "ocr", feature = "tokio-runtime")))]
            {
                let _ = config; // suppress unused warning when OCR is disabled
                Some(extracted_images)
            }
        } else {
            None
        };

        // Build document structure from already-parsed notebook (no re-parse)
        let document = if config.include_document_structure {
            Self::build_document_structure(&notebook_json)
        } else {
            None
        };

        Ok(ExtractionResult {
            content: extracted_content,
            mime_type: mime_type.to_string().into(),
            metadata: Metadata {
                additional: metadata_additional,
                ..Default::default()
            },
            pages: None,
            tables: vec![],
            detected_languages: None,
            chunks: None,
            images,
            djot_content: None,
            elements: None,
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
        &["application/x-ipynb+json"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jupyter_extractor_plugin_interface() {
        let extractor = JupyterExtractor::new();
        assert_eq!(extractor.name(), "jupyter-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 50);
        assert!(extractor.supported_mime_types().contains(&"application/x-ipynb+json"));
    }
}

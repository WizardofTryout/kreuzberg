//! Native Rust LaTeX text extractor.
//!
//! This extractor provides comprehensive LaTeX document parsing and text extraction.
//!
//! Features:
//! - Metadata extraction: title, author, date from \title{}, \author{}, \date{}
//! - Section hierarchy: \section{}, \subsection{}, \subsubsection{}, etc.
//! - Inline formatting: \emph{}, \textbf{}, \textit{}, \texttt{}, \underline{}
//! - Lists: itemize, enumerate, description environments
//! - Tables: tabular environment parsing
//! - Math: inline ($...$) and display (\[...\]) math preservation
//! - Unicode support
//!
//! Requires the `office` feature.

mod commands;
mod environments;
mod metadata;
mod parser;
mod utilities;

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::plugins::{DocumentExtractor, Plugin};
use crate::types::builder::DocumentStructureBuilder;
use crate::types::document_structure::DocumentStructure;
use crate::types::{ExtractionResult, Metadata, Table};
use async_trait::async_trait;

use parser::LatexParser;
use utilities::{collect_environment, extract_env_name, extract_heading_title};

/// LaTeX document extractor
pub struct LatexExtractor;

impl LatexExtractor {
    /// Create a new LaTeX extractor.
    pub fn new() -> Self {
        Self
    }

    /// Parse LaTeX content and extract text.
    fn extract_from_latex(content: &str) -> (String, Metadata, Vec<Table>) {
        let mut parser = LatexParser::new(content);
        parser.parse()
    }

    /// Build a `DocumentStructure` from LaTeX source via a lightweight second pass.
    fn build_document_structure(source: &str) -> DocumentStructure {
        let mut builder = DocumentStructureBuilder::new().source_format("latex");
        let lines: Vec<&str> = source.lines().collect();
        let mut in_document = false;
        let is_plain_tex = source.contains("\\bye") && !source.contains("\\begin{document}");
        if is_plain_tex {
            in_document = true;
        }
        // When the document contains \chapter, use absolute LaTeX hierarchy
        // (chapter=1, section=2, ...). Otherwise, section starts at level 1.
        let has_chapters = source.contains("\\chapter{") || source.contains("\\chapter*{");
        let heading_commands: &[(&str, u8)] = if has_chapters {
            &[
                ("chapter*", 1),
                ("chapter", 1),
                ("section*", 2),
                ("section", 2),
                ("subsection*", 3),
                ("subsection", 3),
                ("subsubsection*", 4),
                ("subsubsection", 4),
                ("paragraph*", 5),
                ("paragraph", 5),
            ]
        } else {
            &[
                ("section*", 1),
                ("section", 1),
                ("subsection*", 2),
                ("subsection", 2),
                ("subsubsection*", 3),
                ("subsubsection", 3),
                ("paragraph*", 4),
                ("paragraph", 4),
            ]
        };

        let mut i = 0;

        while i < lines.len() {
            let trimmed = lines[i].trim();

            if is_plain_tex && trimmed.contains("\\bye") {
                break;
            }

            if !is_plain_tex && trimmed.contains("\\begin{document}") {
                in_document = true;
                i += 1;
                continue;
            }

            if !is_plain_tex && trimmed.contains("\\end{document}") {
                break;
            }

            if !in_document {
                i += 1;
                continue;
            }

            // Handle environments
            if (trimmed.contains("\\begin{") || trimmed.contains("\\begin {"))
                && let Some(env_name) = extract_env_name(trimmed)
            {
                match env_name.as_str() {
                    "itemize" | "enumerate" | "description" => {
                        let ordered = env_name == "enumerate";
                        let (env_content, new_i) = collect_environment(&lines, i, &env_name);
                        let list_idx = builder.push_list(ordered, None);
                        Self::build_list_items(&mut builder, &env_content, list_idx);
                        i = new_i;
                        continue;
                    }
                    "tabular" => {
                        let (env_content, new_i) = collect_environment(&lines, i, "tabular");
                        let cells = Self::parse_tabular_cells(&env_content);
                        if !cells.is_empty() {
                            builder.push_table_simple(&cells, None);
                        }
                        i = new_i;
                        continue;
                    }
                    "table" => {
                        let (env_content, new_i) = collect_environment(&lines, i, "table");
                        // Extract tabular inside table environment
                        let end_tag = "\\end{tabular}";
                        if env_content.contains("\\begin{tabular}")
                            && let Some(start) = env_content.find("\\begin{tabular}")
                            && let Some(end) = env_content.find(end_tag)
                        {
                            let tabular_content = &env_content[start..end + end_tag.len()];
                            let inner_lines: Vec<&str> = tabular_content.lines().collect();
                            let (inner_content, _) = collect_environment(&inner_lines, 0, "tabular");
                            let cells = Self::parse_tabular_cells(&inner_content);
                            if !cells.is_empty() {
                                builder.push_table_simple(&cells, None);
                            }
                        }
                        i = new_i;
                        continue;
                    }
                    "equation" | "equation*" | "align" | "align*" | "gather" | "gather*" | "multline" | "multline*"
                    | "eqnarray" | "eqnarray*" | "math" | "displaymath" | "flalign" | "flalign*" | "cases" => {
                        let (env_content, new_i) = collect_environment(&lines, i, &env_name);
                        let formula_text = format!("\\begin{{{}}}\n{}\\end{{{}}}", env_name, env_content, env_name);
                        builder.push_formula(&formula_text, None);
                        i = new_i;
                        continue;
                    }
                    "lstlisting" | "verbatim" | "minted" => {
                        let (env_content, new_i) = collect_environment(&lines, i, &env_name);
                        // Try to extract language from lstlisting options
                        let language = if env_name == "lstlisting" || env_name == "minted" {
                            Self::extract_code_language(trimmed)
                        } else {
                            None
                        };
                        builder.push_code(env_content.trim(), language.as_deref(), None);
                        i = new_i;
                        continue;
                    }
                    _ => {
                        // Skip other environments
                        let (_, new_i) = collect_environment(&lines, i, &env_name);
                        i = new_i;
                        continue;
                    }
                }
            }

            // Handle heading commands

            let mut handled = false;
            for &(cmd, level) in heading_commands {
                let cmd_prefix = format!("\\{}", cmd);
                if trimmed.starts_with(&cmd_prefix) {
                    let rest = &trimmed[cmd_prefix.len()..];
                    if rest.starts_with('{') || rest.starts_with('[') {
                        if let Some(title) = extract_heading_title(trimmed, cmd) {
                            builder.push_heading(level, &title, None, None);
                        }
                        handled = true;
                        break;
                    }
                }
            }

            if !handled && !trimmed.is_empty() && !trimmed.starts_with('%') {
                // Handle display math \[...\]
                if trimmed.starts_with("\\[") {
                    let mut math_content = trimmed.to_string();
                    if !trimmed.contains("\\]") {
                        i += 1;
                        while i < lines.len() {
                            math_content.push('\n');
                            math_content.push_str(lines[i]);
                            if lines[i].trim().contains("\\]") {
                                break;
                            }
                            i += 1;
                        }
                    }
                    let formula = math_content.trim_start_matches("\\[").trim_end_matches("\\]").trim();
                    if !formula.is_empty() {
                        builder.push_formula(formula, None);
                    }
                } else if trimmed.contains('$') && !trimmed.starts_with('\\') {
                    // Lines containing inline math - treat as paragraph
                    builder.push_paragraph(trimmed, vec![], None, None);
                } else if !trimmed.starts_with('\\')
                    || trimmed.starts_with("\\textbf")
                    || trimmed.starts_with("\\emph")
                    || trimmed.starts_with("\\textit")
                {
                    // Regular text content
                    builder.push_paragraph(trimmed, vec![], None, None);
                }
            }

            i += 1;
        }

        builder.build()
    }

    /// Parse tabular cells from environment content.
    fn parse_tabular_cells(content: &str) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("\\hline")
                || trimmed.is_empty()
                || trimmed.contains("\\begin{tabular}")
                || trimmed.contains("\\end{tabular}")
            {
                continue;
            }
            let row_str = trimmed.replace("\\\\", "");
            let cells: Vec<String> = row_str
                .split('&')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !cells.is_empty() {
                rows.push(cells);
            }
        }
        rows
    }

    /// Build list items from a list environment's content.
    fn build_list_items(
        builder: &mut DocumentStructureBuilder,
        content: &str,
        list_idx: crate::types::document_structure::NodeIndex,
    ) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("\\item") {
                let after = trimmed.strip_prefix("\\item").unwrap_or("").trim();
                // Handle \item[label] for description lists
                let text = if after.starts_with('[') {
                    if let Some(bracket_end) = after.find(']') {
                        let label = &after[1..bracket_end];
                        let rest = after[bracket_end + 1..].trim();
                        format!("{}: {}", label, rest)
                    } else {
                        after.to_string()
                    }
                } else {
                    after.to_string()
                };
                if !text.is_empty() {
                    builder.push_list_item(list_idx, &text, None);
                }
            }
        }
    }

    /// Extract language from code environment options.
    fn extract_code_language(begin_line: &str) -> Option<String> {
        // \begin{lstlisting}[language=Python] or \begin{minted}{python}
        if let Some(lang_pos) = begin_line.find("language=") {
            let after = &begin_line[lang_pos + 9..];
            let end = after.find([',', ']', '}']).unwrap_or(after.len());
            let lang = after[..end].trim().to_string();
            if !lang.is_empty() {
                return Some(lang);
            }
        }
        // \begin{minted}{python}
        if begin_line.contains("minted")
            && let Some(brace_start) = begin_line.rfind('{')
        {
            let after = &begin_line[brace_start + 1..];
            if let Some(brace_end) = after.find('}') {
                let lang = after[..brace_end].trim().to_string();
                if !lang.is_empty() && lang != "minted" {
                    return Some(lang);
                }
            }
        }
        None
    }
}

impl Default for LatexExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for LatexExtractor {
    fn name(&self) -> &str {
        "latex-extractor"
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
        "Native Rust LaTeX document extractor with metadata and table support"
    }

    fn author(&self) -> &str {
        "Kreuzberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl DocumentExtractor for LatexExtractor {
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
        let latex_str = String::from_utf8_lossy(content).to_string();
        let (text, metadata, tables) = Self::extract_from_latex(&latex_str);
        let document = if config.include_document_structure {
            Some(Self::build_document_structure(&latex_str))
        } else {
            None
        };

        Ok(ExtractionResult {
            content: text,
            mime_type: mime_type.to_string().into(),
            metadata,
            tables,
            detected_languages: None,
            chunks: None,
            images: None,
            djot_content: None,
            pages: None,
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
        &["application/x-latex", "text/x-tex"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_title_extraction() {
        let latex = r#"\title{Hello World}"#;
        let (_, metadata, _) = LatexExtractor::extract_from_latex(latex);
        assert_eq!(
            metadata.additional.get("title").and_then(|v| v.as_str()),
            Some("Hello World")
        );
    }

    #[test]
    fn test_author_extraction() {
        let latex = r#"\author{John Doe}"#;
        let (_, metadata, _) = LatexExtractor::extract_from_latex(latex);
        assert!(metadata.additional.contains_key("author"));
    }

    #[test]
    fn test_section_extraction() {
        let latex = r#"\begin{document}\section{Introduction}\end{document}"#;
        let (content, _, _) = LatexExtractor::extract_from_latex(latex);
        assert!(content.contains("Introduction"));
    }
}

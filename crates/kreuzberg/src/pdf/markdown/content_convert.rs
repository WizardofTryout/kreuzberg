//! Convert [`PageContent`] elements into [`PdfParagraph`]s for the markdown pipeline.
//!
//! This is the shared conversion layer: all extraction backends produce
//! `PageContent` via adapters, then this module converts elements into
//! the `PdfParagraph` representation used by heading classification,
//! layout overrides, and markdown rendering.

use super::constants::MAX_HEADING_WORD_COUNT;
use super::content::{ContentElement, ElementLevel, PageContent, SemanticRole};
use super::types::{LayoutHintClass, PdfLine, PdfParagraph};
use crate::pdf::hierarchy::SegmentData;

/// Y-proximity tolerance as a fraction of median element height, for line grouping.
const LINE_Y_TOLERANCE_FRACTION: f32 = 0.5;

/// Convert a page's content elements into paragraphs.
///
/// For word-level OCR content (majority of elements are `ElementLevel::Word`),
/// spatially proximate words are grouped into lines and then into paragraphs.
/// For block/line-level content, each element becomes its own paragraph.
pub(super) fn content_to_paragraphs(page: &PageContent) -> Vec<PdfParagraph> {
    // Check if the majority of elements are word-level.
    let word_count = page
        .elements
        .iter()
        .filter(|e| e.level == ElementLevel::Word)
        .count();
    let total = page.elements.len();

    if total > 0 && word_count > total / 2 {
        return group_words_to_paragraphs(&page.elements);
    }

    let mut paragraphs = Vec::with_capacity(total);
    for elem in &page.elements {
        if let Some(para) = element_to_paragraph(elem) {
            paragraphs.push(para);
        }
    }
    paragraphs
}

/// Group word-level elements into multi-word, multi-line paragraphs.
///
/// Algorithm:
/// 1. Sort by Y position (top-to-bottom in PDF coords = largest y_min first)
/// 2. Group into lines by y_min proximity (tolerance = median height × 0.5)
/// 3. Sort within lines by X position (left-to-right)
/// 4. Group lines into paragraphs by vertical gap (gap > 1.5× median line height)
/// 5. Create one PdfParagraph per paragraph group
fn group_words_to_paragraphs(elements: &[ContentElement]) -> Vec<PdfParagraph> {
    if elements.is_empty() {
        return Vec::new();
    }

    // --- Step 1: compute median element height for Y tolerance ---
    let mut heights: Vec<f32> = elements
        .iter()
        .filter_map(|e| e.bbox.map(|r| r.height()))
        .filter(|h| *h > 0.0)
        .collect();

    let median_height = if !heights.is_empty() {
        heights.sort_by(|a, b| a.total_cmp(b));
        heights[heights.len() / 2]
    } else {
        12.0
    };
    let tolerance = median_height * LINE_Y_TOLERANCE_FRACTION;

    // --- Step 2: sort elements top-to-bottom (largest y_min first in PDF coords),
    //             then left-to-right within the same row ---
    let mut sorted_indices: Vec<usize> = (0..elements.len()).collect();
    sorted_indices.sort_by(|&a, &b| {
        let y_a = elements[a].bbox.map_or(0.0, |r| r.y_min);
        let y_b = elements[b].bbox.map_or(0.0, |r| r.y_min);
        let x_a = elements[a].bbox.map_or(0.0, |r| r.left);
        let x_b = elements[b].bbox.map_or(0.0, |r| r.left);
        // Descending y (top of page = highest y_min in PDF space)
        y_b.total_cmp(&y_a).then_with(|| x_a.total_cmp(&x_b))
    });

    // --- Step 3: group sorted elements into lines by y_min proximity ---
    let mut lines: Vec<Vec<usize>> = Vec::new(); // each entry = list of element indices
    let mut current_line: Vec<usize> = Vec::new();
    let mut line_y_sum: f32 = 0.0;

    for &idx in &sorted_indices {
        let y = elements[idx].bbox.map_or(0.0, |r| r.y_min);

        if current_line.is_empty() {
            current_line.push(idx);
            line_y_sum = y;
        } else {
            let avg_y = line_y_sum / current_line.len() as f32;
            if (y - avg_y).abs() <= tolerance {
                current_line.push(idx);
                line_y_sum += y;
            } else {
                // Finalize this line, sorted left-to-right
                current_line.sort_by(|&a, &b| {
                    let xa = elements[a].bbox.map_or(0.0, |r| r.left);
                    let xb = elements[b].bbox.map_or(0.0, |r| r.left);
                    xa.total_cmp(&xb)
                });
                lines.push(current_line);
                current_line = vec![idx];
                line_y_sum = y;
            }
        }
    }
    if !current_line.is_empty() {
        current_line.sort_by(|&a, &b| {
            let xa = elements[a].bbox.map_or(0.0, |r| r.left);
            let xb = elements[b].bbox.map_or(0.0, |r| r.left);
            xa.total_cmp(&xb)
        });
        lines.push(current_line);
    }

    if lines.is_empty() {
        return Vec::new();
    }

    // --- Step 4: compute median line height for paragraph break detection ---
    let line_heights: Vec<f32> = lines
        .iter()
        .map(|line| {
            let min_y = line
                .iter()
                .filter_map(|&i| elements[i].bbox.map(|r| r.y_min))
                .fold(f32::MAX, f32::min);
            let max_y = line
                .iter()
                .filter_map(|&i| elements[i].bbox.map(|r| r.y_max))
                .fold(f32::MIN, f32::max);
            if min_y == f32::MAX || max_y == f32::MIN {
                median_height
            } else {
                (max_y - min_y).max(1.0)
            }
        })
        .collect();

    let median_line_height = {
        let mut sorted = line_heights.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        sorted[sorted.len() / 2]
    };

    // --- Step 5: group lines into paragraphs ---
    let mut paragraphs: Vec<PdfParagraph> = Vec::new();
    let mut current_para_lines: Vec<&Vec<usize>> = Vec::new();
    // Track the y_min (bottom edge) of the previous line to measure inter-line gap.
    let mut prev_line_bottom: Option<f32> = None;

    for (line_idx, line) in lines.iter().enumerate() {
        // y_max is the top edge of the line in PDF coords
        let line_top = line
            .iter()
            .filter_map(|&i| elements[i].bbox.map(|r| r.y_max))
            .fold(f32::MIN, f32::max);
        // y_min is the bottom edge
        let line_bottom = line
            .iter()
            .filter_map(|&i| elements[i].bbox.map(|r| r.y_min))
            .fold(f32::MAX, f32::min);

        // Detect paragraph break: gap between the bottom of the previous line
        // and the top of this line. In PDF space both are positive Y values with
        // y increasing upward, so the previous line sits higher (larger y) than
        // the current line.  gap = prev_line_bottom - line_top (positive when
        // there is vertical white space between them).
        if let Some(prev_bottom) = prev_line_bottom {
            let gap = prev_bottom - line_top;
            if gap > median_line_height * 1.5 && !current_para_lines.is_empty() {
                if let Some(para) = build_paragraph_from_lines(&current_para_lines, elements) {
                    paragraphs.push(para);
                }
                current_para_lines = Vec::new();
            }
        }

        current_para_lines.push(line);
        // Record the bottom of this line (y_min) for next iteration.
        prev_line_bottom = Some(if line_bottom == f32::MAX {
            line_top - line_heights[line_idx]
        } else {
            line_bottom
        });
    }

    if !current_para_lines.is_empty() {
        if let Some(para) = build_paragraph_from_lines(&current_para_lines, elements) {
            paragraphs.push(para);
        }
    }

    paragraphs
}

/// Build a single `PdfParagraph` from a group of lines (each line is a slice of element indices).
fn build_paragraph_from_lines(
    line_groups: &[&Vec<usize>],
    elements: &[ContentElement],
) -> Option<PdfParagraph> {
    // Use the first line's first element for semantic properties.
    let first_elem = line_groups
        .first()
        .and_then(|l| l.first())
        .map(|&i| &elements[i]);

    // Compute dominant (most common) font size across all words.
    let dominant_font_size = {
        let mut sizes: Vec<f32> = line_groups
            .iter()
            .flat_map(|l| l.iter())
            .filter_map(|&i| elements[i].font_size)
            .collect();
        if sizes.is_empty() {
            first_elem.and_then(|e| e.font_size).unwrap_or(12.0)
        } else {
            sizes.sort_by(|a, b| a.total_cmp(b));
            // Median as a simple approximation of dominant size.
            sizes[sizes.len() / 2]
        }
    };

    // Build the PdfLine entries.
    let mut pdf_lines: Vec<PdfLine> = Vec::new();
    let mut total_word_count = 0usize;

    for line in line_groups {
        let mut segments: Vec<SegmentData> = Vec::new();
        let mut line_is_bold = false;
        let mut line_is_monospace = false;
        let mut baseline_y_sum = 0.0f32;
        let mut baseline_count = 0usize;

        for &idx in line.iter() {
            let elem = &elements[idx];
            let text = elem.text.trim();
            if text.is_empty() {
                continue;
            }
            let font_size = elem.font_size.unwrap_or(dominant_font_size);
            let is_code = matches!(elem.semantic_role, Some(SemanticRole::Code));
            let is_monospace = elem.is_monospace || is_code;

            if elem.is_bold {
                line_is_bold = true;
            }
            if is_monospace {
                line_is_monospace = true;
            }

            let y_min = elem.bbox.map_or(0.0, |r| r.y_min);
            baseline_y_sum += y_min;
            baseline_count += 1;

            segments.push(SegmentData {
                text: text.to_string(),
                x: elem.bbox.map_or(0.0, |r| r.left),
                y: y_min,
                width: elem.bbox.map_or(0.0, |r| r.width()),
                height: elem.bbox.map_or(0.0, |r| r.height()),
                font_size,
                is_bold: elem.is_bold,
                is_italic: elem.is_italic,
                is_monospace,
                baseline_y: y_min,
            });

            total_word_count += 1;
        }

        if segments.is_empty() {
            continue;
        }

        let avg_baseline = if baseline_count > 0 {
            baseline_y_sum / baseline_count as f32
        } else {
            0.0
        };

        pdf_lines.push(PdfLine {
            segments,
            baseline_y: avg_baseline,
            dominant_font_size,
            is_bold: line_is_bold,
            is_monospace: line_is_monospace,
        });
    }

    if total_word_count == 0 {
        return None;
    }

    // Derive paragraph properties from the first element.
    let (heading_level, is_list_item, is_code_block, is_formula, is_bold, is_page_furniture, layout_class) =
        if let Some(elem) = first_elem {
            let is_code = matches!(elem.semantic_role, Some(SemanticRole::Code));
            let is_formula = matches!(elem.semantic_role, Some(SemanticRole::Formula))
                || matches!(elem.layout_class, Some(LayoutHintClass::Formula));
            let is_page_furniture = matches!(
                elem.semantic_role,
                Some(SemanticRole::PageHeader) | Some(SemanticRole::PageFooter)
            );
            let mut is_list = matches!(elem.semantic_role, Some(SemanticRole::ListItem));

            // Detect list items from text content when not tagged.
            if !is_list {
                let first_word = pdf_lines
                    .first()
                    .and_then(|l| l.segments.first())
                    .map(|s| s.text.as_str())
                    .unwrap_or("");
                is_list = super::paragraphs::is_list_prefix(first_word);
            }

            let heading_level = match elem.semantic_role {
                Some(SemanticRole::Heading { level }) if total_word_count <= MAX_HEADING_WORD_COUNT => {
                    Some(level)
                }
                _ => None,
            };

            (heading_level, is_list, is_code, is_formula, elem.is_bold, is_page_furniture, elem.layout_class)
        } else {
            (None, false, false, false, false, false, None)
        };

    // Compute block_bbox spanning all words in the paragraph.
    let block_bbox = {
        let mut left = f32::MAX;
        let mut bottom = f32::MAX;
        let mut right = f32::MIN;
        let mut top = f32::MIN;
        for line in line_groups {
            for &idx in line.iter() {
                if let Some(r) = elements[idx].bbox {
                    left = left.min(r.left);
                    bottom = bottom.min(r.y_min);
                    right = right.max(r.right);
                    top = top.max(r.y_max);
                }
            }
        }
        if left == f32::MAX {
            None
        } else {
            Some((left, bottom, right, top))
        }
    };

    Some(PdfParagraph {
        lines: pdf_lines,
        dominant_font_size,
        heading_level,
        is_bold,
        is_list_item,
        is_code_block,
        is_formula,
        is_page_furniture,
        layout_class,
        caption_for: None,
        block_bbox,
    })
}

/// Convert a single `ContentElement` into a `PdfParagraph`.
///
/// Returns `None` for empty elements.
fn element_to_paragraph(elem: &ContentElement) -> Option<PdfParagraph> {
    // Build the full text, prepending list label if present.
    let full_text = if let Some(ref label) = elem.list_label {
        format!("{} {}", label, elem.text)
    } else {
        elem.text.clone()
    };

    let word_count = full_text.split_whitespace().count();
    if word_count == 0 {
        return None;
    }

    let font_size = elem.font_size.unwrap_or(12.0);

    // Determine structural properties from semantic role.
    let mut is_list_item = matches!(elem.semantic_role, Some(SemanticRole::ListItem));
    let is_code_block = matches!(elem.semantic_role, Some(SemanticRole::Code));
    let is_formula = matches!(elem.semantic_role, Some(SemanticRole::Formula))
        || matches!(elem.layout_class, Some(LayoutHintClass::Formula));
    let is_monospace = elem.is_monospace || is_code_block;
    let is_page_furniture = matches!(
        elem.semantic_role,
        Some(SemanticRole::PageHeader) | Some(SemanticRole::PageFooter)
    );

    // Detect list items from text content when not tagged.
    if !is_list_item {
        let first_word = full_text.split_whitespace().next().unwrap_or("");
        is_list_item = super::paragraphs::is_list_prefix(first_word);
    }

    // Map heading level from semantic role, with word-count guard.
    let heading_level = match elem.semantic_role {
        Some(SemanticRole::Heading { level }) if word_count <= MAX_HEADING_WORD_COUNT => Some(level),
        _ => None,
    };

    // Extract block_bbox as (left, bottom, right, top) tuple for PdfParagraph.
    let block_bbox = elem.bbox.map(|r| (r.left, r.y_min, r.right, r.y_max));

    // Create word-level segments (zero positions — spatial matching uses block_bbox).
    let segments: Vec<SegmentData> = if elem.level == ElementLevel::Line || elem.level == ElementLevel::Block {
        // Block/line-level elements: split into word segments.
        full_text
            .split_whitespace()
            .map(|w| SegmentData {
                text: w.to_string(),
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
                font_size,
                is_bold: elem.is_bold,
                is_italic: elem.is_italic,
                is_monospace,
                baseline_y: 0.0,
            })
            .collect()
    } else {
        // Word-level elements: single segment.
        vec![SegmentData {
            text: full_text.clone(),
            x: elem.bbox.map_or(0.0, |r| r.left),
            y: elem.bbox.map_or(0.0, |r| r.y_min),
            width: elem.bbox.map_or(0.0, |r| r.width()),
            height: elem.bbox.map_or(0.0, |r| r.height()),
            font_size,
            is_bold: elem.is_bold,
            is_italic: elem.is_italic,
            is_monospace,
            baseline_y: elem.bbox.map_or(0.0, |r| r.y_min),
        }]
    };

    let line = PdfLine {
        segments,
        baseline_y: 0.0,
        dominant_font_size: font_size,
        is_bold: elem.is_bold,
        is_monospace,
    };

    Some(PdfParagraph {
        lines: vec![line],
        dominant_font_size: font_size,
        heading_level,
        is_bold: elem.is_bold,
        is_list_item,
        is_code_block,
        is_formula,
        is_page_furniture,
        layout_class: elem.layout_class,
        caption_for: None,
        block_bbox,
    })
}

#[cfg(test)]
mod tests {
    use super::super::content::ExtractionSource;
    use super::super::geometry::Rect;
    use super::*;

    fn make_element(text: &str, role: Option<SemanticRole>) -> ContentElement {
        ContentElement {
            text: text.to_string(),
            bbox: None,
            font_size: Some(12.0),
            is_bold: false,
            is_italic: false,
            is_monospace: false,
            confidence: None,
            semantic_role: role,
            level: ElementLevel::Block,
            list_label: None,
            layout_class: None,
        }
    }

    fn make_word(text: &str, x: f32, y_min: f32, y_max: f32) -> ContentElement {
        ContentElement {
            text: text.to_string(),
            bbox: Some(Rect::from_lbrt(x, y_min, x + 30.0, y_max)),
            font_size: Some(12.0),
            is_bold: false,
            is_italic: false,
            is_monospace: false,
            confidence: None,
            semantic_role: Some(SemanticRole::Paragraph),
            level: ElementLevel::Word,
            list_label: None,
            layout_class: None,
        }
    }

    fn make_page(elements: Vec<ContentElement>) -> PageContent {
        PageContent {
            page_number: 1,
            page_width: 612.0,
            page_height: 792.0,
            elements,
            source: ExtractionSource::StructureTree,
        }
    }

    #[test]
    fn test_heading_conversion() {
        let page = make_page(vec![
            make_element("Title Text", Some(SemanticRole::Heading { level: 1 })),
            make_element("Body text", Some(SemanticRole::Paragraph)),
        ]);
        let paras = content_to_paragraphs(&page);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].heading_level, Some(1));
        assert_eq!(paras[1].heading_level, None);
    }

    #[test]
    fn test_heading_too_many_words_demoted() {
        let long_heading = (0..25).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ");
        let page = make_page(vec![make_element(
            &long_heading,
            Some(SemanticRole::Heading { level: 2 }),
        )]);
        let paras = content_to_paragraphs(&page);
        assert_eq!(paras[0].heading_level, None);
    }

    #[test]
    fn test_list_item_from_role() {
        let mut elem = make_element("First item", Some(SemanticRole::ListItem));
        elem.list_label = Some("1.".to_string());
        let page = make_page(vec![elem]);
        let paras = content_to_paragraphs(&page);
        assert!(paras[0].is_list_item);
        assert_eq!(paras[0].lines[0].segments[0].text, "1.");
    }

    #[test]
    fn test_list_item_from_text_prefix() {
        let page = make_page(vec![make_element("• Bullet point", Some(SemanticRole::Paragraph))]);
        let paras = content_to_paragraphs(&page);
        assert!(paras[0].is_list_item);
    }

    #[test]
    fn test_code_block() {
        let page = make_page(vec![make_element("fn main() {}", Some(SemanticRole::Code))]);
        let paras = content_to_paragraphs(&page);
        assert!(paras[0].is_code_block);
    }

    #[test]
    fn test_empty_skipped() {
        let page = make_page(vec![
            make_element("", Some(SemanticRole::Paragraph)),
            make_element("   ", Some(SemanticRole::Paragraph)),
            make_element("Real text", Some(SemanticRole::Paragraph)),
        ]);
        let paras = content_to_paragraphs(&page);
        assert_eq!(paras.len(), 1);
        assert_eq!(paras[0].lines[0].segments[0].text, "Real");
    }

    #[test]
    fn test_block_bbox_propagated() {
        let mut elem = make_element("With bounds", Some(SemanticRole::Paragraph));
        elem.bbox = Some(Rect::from_lbrt(50.0, 100.0, 400.0, 120.0));
        let page = make_page(vec![elem]);
        let paras = content_to_paragraphs(&page);
        let bbox = paras[0].block_bbox.unwrap();
        assert!((bbox.0 - 50.0).abs() < f32::EPSILON);
        assert!((bbox.1 - 100.0).abs() < f32::EPSILON);
    }

    // --- Word grouping tests ---

    #[test]
    fn test_six_words_two_lines_one_paragraph() {
        // Line 1: y_min=700, y_max=712 — three words side by side (median height = 12)
        // Line 2: y_min=684, y_max=696 — three words (gap = 700 - 696 = 4 < 1.5×12 = 18)
        let elements = vec![
            make_word("Hello", 50.0, 700.0, 712.0),
            make_word("world", 85.0, 700.0, 712.0),
            make_word("foo", 120.0, 700.0, 712.0),
            make_word("bar", 50.0, 684.0, 696.0),
            make_word("baz", 85.0, 684.0, 696.0),
            make_word("qux", 120.0, 684.0, 696.0),
        ];
        let page = PageContent {
            page_number: 1,
            page_width: 612.0,
            page_height: 792.0,
            elements,
            source: ExtractionSource::Ocr,
        };
        let paras = content_to_paragraphs(&page);
        assert_eq!(paras.len(), 1, "expected 1 paragraph, got {}", paras.len());
        assert_eq!(paras[0].lines.len(), 2, "expected 2 lines, got {}", paras[0].lines.len());
        // First line should have 3 segments in left-to-right order.
        assert_eq!(paras[0].lines[0].segments.len(), 3);
        assert_eq!(paras[0].lines[0].segments[0].text, "Hello");
        assert_eq!(paras[0].lines[0].segments[1].text, "world");
        assert_eq!(paras[0].lines[0].segments[2].text, "foo");
    }

    #[test]
    fn test_large_gap_produces_two_paragraphs() {
        // Line 1: y_min=700, y_max=712 (median height = 12)
        // Line 2: y_min=600, y_max=612 — gap = 700 - 612 = 88 > 1.5×12 = 18 → new paragraph
        let elements = vec![
            make_word("First", 50.0, 700.0, 712.0),
            make_word("para", 85.0, 700.0, 712.0),
            make_word("Second", 50.0, 600.0, 612.0),
            make_word("para", 85.0, 600.0, 612.0),
        ];
        let page = PageContent {
            page_number: 1,
            page_width: 612.0,
            page_height: 792.0,
            elements,
            source: ExtractionSource::Ocr,
        };
        let paras = content_to_paragraphs(&page);
        assert_eq!(paras.len(), 2, "expected 2 paragraphs, got {}", paras.len());
        assert_eq!(paras[0].lines[0].segments[0].text, "First");
        assert_eq!(paras[1].lines[0].segments[0].text, "Second");
    }

    #[test]
    fn test_single_word_produces_one_paragraph() {
        let elements = vec![make_word("Solo", 50.0, 400.0, 412.0)];
        let page = PageContent {
            page_number: 1,
            page_width: 612.0,
            page_height: 792.0,
            elements,
            source: ExtractionSource::Ocr,
        };
        let paras = content_to_paragraphs(&page);
        assert_eq!(paras.len(), 1);
        assert_eq!(paras[0].lines[0].segments[0].text, "Solo");
    }

    #[test]
    fn test_empty_word_elements_skipped() {
        let mut empty = make_word("", 50.0, 400.0, 412.0);
        empty.text = "   ".to_string();
        let page = PageContent {
            page_number: 1,
            page_width: 612.0,
            page_height: 792.0,
            elements: vec![empty, make_word("Real", 85.0, 400.0, 412.0)],
            source: ExtractionSource::Ocr,
        };
        let paras = content_to_paragraphs(&page);
        assert_eq!(paras.len(), 1);
        assert_eq!(paras[0].lines[0].segments[0].text, "Real");
    }

    #[test]
    fn test_block_bbox_spans_all_words_in_paragraph() {
        // A = (50, 700, 80, 712), B = (200, 700, 230, 712), C = (100, 685, 130, 697)
        // All within tolerance of each other so they form one paragraph.
        // Expected bbox: left=50, bottom=685, right=230, top=712
        let elements = vec![
            make_word("A", 50.0, 700.0, 712.0),
            make_word("B", 200.0, 700.0, 712.0),
            make_word("C", 100.0, 685.0, 697.0),
        ];
        let page = PageContent {
            page_number: 1,
            page_width: 612.0,
            page_height: 792.0,
            elements,
            source: ExtractionSource::Ocr,
        };
        let paras = content_to_paragraphs(&page);
        assert_eq!(paras.len(), 1);
        let bbox = paras[0].block_bbox.unwrap();
        assert!((bbox.0 - 50.0).abs() < f32::EPSILON, "left={}", bbox.0);
        assert!((bbox.1 - 685.0).abs() < f32::EPSILON, "bottom={}", bbox.1);
        assert!((bbox.2 - 230.0).abs() < f32::EPSILON, "right={}", bbox.2);
        assert!((bbox.3 - 712.0).abs() < f32::EPSILON, "top={}", bbox.3);
    }
}

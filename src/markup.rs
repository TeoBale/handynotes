use std::ops::Range;

use gpui_kit::{FontStyle, FontWeight, HighlightStyle, StrikethroughStyle, combine_highlights, px};
use markdown::{ParseOptions, mdast::Node, to_mdast};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Bold,
    Italic,
    Strike,
}

impl Format {
    pub fn for_key(key: &str) -> Option<Self> {
        match key {
            "b" => Some(Self::Bold),
            "i" => Some(Self::Italic),
            "s" => Some(Self::Strike),
            _ => None,
        }
    }

    fn delimiter(self) -> &'static str {
        match self {
            Self::Bold => "**",
            Self::Italic => "_",
            Self::Strike => "~~",
        }
    }

    fn style(self) -> HighlightStyle {
        let mut style = HighlightStyle::default();
        match self {
            Self::Bold => style.font_weight = Some(FontWeight::BOLD),
            Self::Italic => style.font_style = Some(FontStyle::Italic),
            Self::Strike => {
                style.strikethrough = Some(StrikethroughStyle {
                    thickness: px(1.),
                    color: None,
                });
            }
        }
        style
    }
}

struct Span {
    format: Format,
    source: Range<usize>,
    content: Range<usize>,
}

fn spans(text: &str) -> Vec<Span> {
    fn collect(node: &Node, spans: &mut Vec<Span>) {
        let format = match node {
            Node::Strong(_) => Some(Format::Bold),
            Node::Emphasis(_) => Some(Format::Italic),
            Node::Delete(_) => Some(Format::Strike),
            _ => None,
        };
        if let (Some(format), Some(position)) = (format, node.position()) {
            let source = position.start.offset..position.end.offset;
            let width = format.delimiter().len();
            let content = source.start + width..source.end - width;
            spans.push(Span {
                format,
                source,
                content,
            });
        }
        if let Some(children) = node.children() {
            for child in children {
                collect(child, spans);
            }
        }
    }

    let mut spans = Vec::new();
    let mut options = ParseOptions::gfm();
    options.gfm_strikethrough_single_tilde = false;
    if let Ok(root) = to_mdast(text, &options) {
        collect(&root, &mut spans);
    }
    spans
}

pub fn styles(text: &str) -> Vec<(Range<usize>, HighlightStyle)> {
    if !text.contains(['*', '_', '~']) {
        return Vec::new();
    }
    let spans = spans(text)
        .into_iter()
        .map(|span| (span.content, span.format.style()));
    combine_highlights(spans, []).collect()
}

pub fn format_selection(
    text: &str,
    selection: Range<usize>,
    format: Format,
) -> Option<(Range<usize>, String, Range<usize>)> {
    let selection = trim_selection(text, selection)?;
    let spans = spans(text);
    let delimiter = format.delimiter();

    if let Some(span) = matching_span(&spans, &selection, format) {
        let adjusted = if selection == span.source {
            span.source.start..span.source.end - 2 * delimiter.len()
        } else {
            selection.start - delimiter.len()..selection.end - delimiter.len()
        };
        return Some((
            span.source.clone(),
            text[span.content.clone()].into(),
            adjusted,
        ));
    }

    let selected = &text[selection.clone()];
    if !selected.contains('\n') {
        let delimiter = opening_delimiter(text, &selection, format);
        let adjusted = selection.start + delimiter.len()..selection.end + delimiter.len();
        let wrapped = format!("{delimiter}{selected}{delimiter}");
        return Some((selection, wrapped, adjusted));
    }

    // A Markdown inline span cannot cross a blank line or other block boundary.
    let mut offset = selection.start;
    let mut replacement = String::new();
    for line in selected.split_inclusive('\n') {
        let end = offset + line.len();
        if let Some(part) = trim_selection(text, offset..end) {
            replacement.push_str(&text[offset..part.start]);
            if let Some(span) = matching_span(&spans, &part, format)
                && span.source == part
            {
                replacement.push_str(&text[span.content.clone()]);
            } else {
                let delimiter = opening_delimiter(text, &part, format);
                replacement.push_str(delimiter);
                replacement.push_str(&text[part.clone()]);
                replacement.push_str(delimiter);
            }
            replacement.push_str(&text[part.end..end]);
        } else {
            replacement.push_str(line);
        }
        offset = end;
    }
    let adjusted = selection.start..selection.start + replacement.len();
    Some((selection, replacement, adjusted))
}

fn opening_delimiter(text: &str, selection: &Range<usize>, format: Format) -> &'static str {
    if format == Format::Italic {
        // CommonMark requires asterisks for emphasis inside words.
        let before = text[..selection.start].chars().next_back();
        let after = text[selection.end..].chars().next();
        if before.is_some_and(char::is_alphanumeric) || after.is_some_and(char::is_alphanumeric) {
            return "*";
        }
    }
    format.delimiter()
}

fn trim_selection(text: &str, selection: Range<usize>) -> Option<Range<usize>> {
    let selected = &text[selection.clone()];
    let start = selection.start + selected.len() - selected.trim_start().len();
    let end = selection.start + selected.trim_end().len();
    (start < end).then_some(start..end)
}

fn matching_span<'a>(
    spans: &'a [Span],
    selection: &Range<usize>,
    format: Format,
) -> Option<&'a Span> {
    spans.iter().find(|span| {
        if span.format != format {
            return false;
        }
        let mut content = &span.content;
        while let Some(nested) = spans.iter().find(|nested| nested.source == *content) {
            if selection == content {
                return true;
            }
            content = &nested.content;
        }
        selection == &span.source || selection == content
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_underscores_and_keeps_emphasis_inside_words() {
        for (text, selection, expected) in [
            ("Café", 0.."Café".len(), "_Café_"),
            ("prefix", 3..6, "pre*fix*"),
        ] {
            let (range, replacement, selected) =
                format_selection(text, selection, Format::Italic).unwrap();
            let mut result = text.to_owned();
            result.replace_range(range, &replacement);
            assert_eq!(result, expected);
            assert_eq!(styles(&result)[0].1.font_style, Some(FontStyle::Italic));

            let (range, replacement, _) =
                format_selection(&result, selected, Format::Italic).unwrap();
            result.replace_range(range, &replacement);
            assert_eq!(result, text);
        }
    }

    #[test]
    fn toggles_nested_emphasis_without_removing_other_formatting() {
        let text = "é ***Café***";
        let start = text.find("Café").unwrap();
        let selection = start..start + "Café".len();

        for (format, expected) in [(Format::Bold, "é *Café*"), (Format::Italic, "é **Café**")] {
            let (range, replacement, selected) =
                format_selection(text, selection.clone(), format).unwrap();
            let mut result = text.to_owned();
            result.replace_range(range, &replacement);
            assert_eq!(result, expected);
            assert_eq!(&result[selected], "Café");
        }
    }

    #[test]
    fn formats_multiple_paragraphs_and_keeps_whitespace_outside_delimiters() {
        let text = " first  \n\nsecond ";
        let (range, replacement, selected) =
            format_selection(text, 0..text.len(), Format::Strike).unwrap();
        let mut result = text.to_owned();
        result.replace_range(range, &replacement);
        assert_eq!(result, " ~~first~~  \n\n~~second~~ ");

        let (range, replacement, _) = format_selection(&result, selected, Format::Strike).unwrap();
        result.replace_range(range, &replacement);
        assert_eq!(result, text);
    }

    #[test]
    fn styles_markdown_but_skips_escaped_markers_and_code() {
        let text = r"é ***Café*** ~~gone~~ `**code**` \*literal\*";
        let styles = styles(text);
        let (range, emphasis) = styles
            .iter()
            .find(|(range, _)| &text[range.clone()] == "Café")
            .unwrap();
        assert_eq!(range.start, text.find("Café").unwrap());
        assert_eq!(emphasis.font_weight, Some(FontWeight::BOLD));
        assert_eq!(emphasis.font_style, Some(FontStyle::Italic));
        assert!(styles.iter().any(|(range, style)| {
            &text[range.clone()] == "gone" && style.strikethrough.is_some()
        }));
        assert!(
            styles
                .iter()
                .all(|(range, _)| range.end <= text.find("`").unwrap())
        );
    }
}

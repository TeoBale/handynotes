use std::{collections::HashMap, ops::Range};

use gpui_kit::{FontStyle, FontWeight, HighlightStyle, StrikethroughStyle, combine_highlights, px};
use linkify::{LinkFinder, LinkKind};
use markdown::{ParseOptions, mdast::Node, to_mdast};

use crate::note_edit::TextEdit;

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

fn spans(root: &Node) -> Vec<Span> {
    nodes(root)
        .filter_map(|node| {
            let format = match node {
                Node::Strong(_) => Format::Bold,
                Node::Emphasis(_) => Format::Italic,
                Node::Delete(_) => Format::Strike,
                _ => return None,
            };
            let position = node.position()?;
            let source = position.start.offset..position.end.offset;
            let width = format.delimiter().len();
            let content = source.start + width..source.end - width;
            Some(Span {
                format,
                source,
                content,
            })
        })
        .collect()
}

pub(crate) fn parse(text: &str) -> Option<Node> {
    let mut options = ParseOptions::gfm();
    options.gfm_strikethrough_single_tilde = false;
    to_mdast(text, &options).ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebLink {
    pub range: Range<usize>,
    pub url: String,
}

pub(crate) fn highlights(
    text: &str,
    root: &Node,
) -> (Vec<(Range<usize>, HighlightStyle)>, Vec<WebLink>) {
    let mut definitions = HashMap::new();
    for node in nodes(root) {
        if let Node::Definition(definition) = node {
            definitions
                .entry(definition.identifier.as_str())
                .or_insert(definition.url.as_str());
        }
    }
    let mut finder = LinkFinder::new();
    finder.url_must_have_scheme(false);
    let mut links = Vec::new();
    let mut link_end = 0;
    for node in nodes(root) {
        let Some(position) = node.position() else {
            continue;
        };
        let range = position.start.offset..position.end.offset;
        if range.start < link_end {
            continue;
        }
        let destination = match node {
            Node::Link(link) => Some(link.url.as_str()),
            Node::LinkReference(reference) => {
                definitions.get(reference.identifier.as_str()).copied()
            }
            Node::Text(_) => {
                for candidate in finder.links(&text[range.clone()]) {
                    if *candidate.kind() != LinkKind::Url || candidate.as_str().contains("://") {
                        continue;
                    }
                    let start = range.start + candidate.start();
                    if text[..start].ends_with(['@', ':', '/', '\\']) {
                        continue;
                    }
                    let Ok(url) = url::Url::parse(&format!("https://{}", candidate.as_str()))
                    else {
                        continue;
                    };
                    if url.domain().is_some_and(|domain| domain.contains('.')) {
                        links.push(WebLink {
                            range: start..range.start + candidate.end(),
                            url: url.into(),
                        });
                    }
                }
                continue;
            }
            _ => continue,
        };
        // The label belongs to its Markdown link, even if its destination
        // cannot be opened. Do not detect a second URL inside that label.
        link_end = range.end;
        if let Some(url) = destination.and_then(|destination| url::Url::parse(destination).ok())
            && matches!(url.scheme(), "http" | "https")
        {
            links.push(WebLink {
                range,
                url: url.into(),
            });
        }
    }
    let inline = spans(root)
        .into_iter()
        .map(|span| (span.content, span.format.style()));
    (combine_highlights(inline, []).collect(), links)
}

pub(crate) fn nodes(root: &Node) -> impl Iterator<Item = &Node> {
    let mut stack = vec![root];
    std::iter::from_fn(move || {
        let node = stack.pop()?;
        if let Some(children) = node.children() {
            stack.extend(children.iter().rev());
        }
        Some(node)
    })
}

pub(crate) fn format_selection(
    text: &str,
    selection: Range<usize>,
    format: Format,
) -> Option<TextEdit> {
    let selection = trim_selection(text, selection)?;
    let root = parse(text)?;
    let spans = spans(&root);
    let delimiter = format.delimiter();

    if let Some(span) = matching_span(&spans, &selection, format) {
        let adjusted = if selection == span.source {
            span.source.start..span.source.end - 2 * delimiter.len()
        } else {
            selection.start - delimiter.len()..selection.end - delimiter.len()
        };
        return Some(TextEdit {
            range: span.source.clone(),
            replacement: text[span.content.clone()].into(),
            selection_after: adjusted,
        });
    }

    let selected_spans: Vec<_> = spans.iter().filter(|span| span.format == format).collect();
    let mut unformatted = Vec::new();
    let mut formatted = Vec::new();
    for part in selected_inline_parts(text, &root, &selection) {
        let mut start = part.start;
        for span in &selected_spans {
            if span.source.end <= start || span.source.start >= part.end {
                continue;
            }
            if let Some(gap) = trim_selection(text, start..start.max(span.source.start)) {
                unformatted.push(gap);
            }
            formatted.push(*span);
            start = part.end.min(span.source.end);
        }
        if let Some(gap) = trim_selection(text, start..part.end) {
            unformatted.push(gap);
        }
    }

    // Apply the style to gaps in a mixed selection. Remove it only once the
    // eligible selection is fully formatted.
    if unformatted.is_empty() {
        let mut edits = Vec::new();
        for span in formatted {
            edits.push(span.source.start..span.content.start);
            edits.push(span.content.end..span.source.end);
        }
        edits.sort_unstable_by_key(|range| range.start);
        edits.dedup();
        let start = selection.start.min(edits.first()?.start);
        let end = selection.end.max(edits.last()?.end);
        let mut adjusted = selection.clone();
        let mut replacement = String::new();
        let mut offset = start;
        for edit in edits {
            replacement.push_str(&text[offset..edit.start]);
            adjusted.start -= removed_before(&edit, selection.start);
            adjusted.end -= removed_before(&edit, selection.end);
            offset = edit.end;
        }
        replacement.push_str(&text[offset..end]);
        return Some(TextEdit {
            range: start..end,
            replacement,
            selection_after: adjusted,
        });
    }

    let mut replacement = String::new();
    let mut offset = selection.start;
    let content_selection = if let [part] = unformatted.as_slice()
        && formatted.is_empty()
        && !text[selection.clone()].contains('\n')
    {
        let width = opening_delimiter(text, part, format).len();
        Some(part.start + width..part.end + width)
    } else {
        None
    };
    for part in unformatted {
        replacement.push_str(&text[offset..part.start]);
        let delimiter = opening_delimiter(text, &part, format);
        replacement.push_str(delimiter);
        replacement.push_str(&text[part.clone()]);
        replacement.push_str(delimiter);
        offset = part.end;
    }
    replacement.push_str(&text[offset..selection.end]);
    let adjusted =
        content_selection.unwrap_or(selection.start..selection.start + replacement.len());
    Some(TextEdit {
        range: selection,
        replacement,
        selection_after: adjusted,
    })
}

fn removed_before(edit: &Range<usize>, offset: usize) -> usize {
    edit.end.min(offset).saturating_sub(edit.start)
}

fn selected_inline_parts(text: &str, root: &Node, selection: &Range<usize>) -> Vec<Range<usize>> {
    let quotes: Vec<_> = nodes(root)
        .filter(|node| matches!(node, Node::Blockquote(_)))
        .filter_map(Node::position)
        .collect();
    let mut parts = Vec::new();
    for block in nodes(root) {
        if !matches!(
            block,
            Node::Heading(_) | Node::Paragraph(_) | Node::TableCell(_)
        ) {
            continue;
        }
        // The inline children exclude heading, list, blockquote, and task markers.
        let Some(children) = block.children() else {
            continue;
        };
        let Some(first) = children.first().and_then(Node::position) else {
            continue;
        };
        let Some(last) = children.last().and_then(Node::position) else {
            continue;
        };
        if first.start.offset >= selection.end || last.end.offset <= selection.start {
            continue;
        }

        let mut offset = first.start.offset;
        for (line_index, line) in text[first.start.offset..last.end.offset]
            .split_inclusive('\n')
            .enumerate()
        {
            let end = offset + line.len();
            let mut start = offset;
            if line_index > 0 {
                // Inline nodes can span several physical lines in a blockquote.
                // A continuation line may omit any of its quote markers.
                let depth = quotes
                    .iter()
                    .filter(|quote| quote.start.offset <= offset && offset < quote.end.offset)
                    .count();
                for _ in 0..depth {
                    let content = text[start..end].trim_start_matches([' ', '\t']);
                    let Some(stripped) = content.strip_prefix('>') else {
                        break;
                    };
                    start = end - stripped.len();
                }
            }
            if matches!(block, Node::Paragraph(_)) {
                start = after_checkbox_marker(text, start, end);
            }
            let start = start.max(selection.start);
            let end_selected = end.min(selection.end);
            if start < end_selected
                && let Some(part) = trim_selection(text, start..end_selected)
            {
                parts.push(part);
            }
            offset = end;
        }
    }
    parts
}

fn after_checkbox_marker(text: &str, start: usize, end: usize) -> usize {
    let content = text[start..end].trim_start_matches([' ', '\t']);
    let remainder = content
        .strip_prefix("[ ]")
        .or_else(|| content.strip_prefix("[x]"))
        .or_else(|| content.strip_prefix("[X]"));
    if let Some(remainder) = remainder
        && (remainder.is_empty() || remainder.starts_with([' ', '\t', '\r', '\n']))
        && text[..start]
            .rsplit('\n')
            .next()
            .is_some_and(|prefix| prefix.trim_matches([' ', '\t']).is_empty())
    {
        end - remainder.len()
    } else {
        start
    }
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

    fn highlights(text: &str) -> (Vec<(Range<usize>, HighlightStyle)>, Vec<WebLink>) {
        super::highlights(text, &parse(text).unwrap())
    }

    #[test]
    fn defaults_to_underscores_and_keeps_emphasis_inside_words() {
        for (text, selection, expected) in [
            ("Café", 0.."Café".len(), "_Café_"),
            ("prefix", 3..6, "pre*fix*"),
        ] {
            let TextEdit {
                range,
                replacement,
                selection_after: selected,
            } = format_selection(text, selection, Format::Italic).unwrap();
            let mut result = text.to_owned();
            result.replace_range(range, &replacement);
            assert_eq!(result, expected);
            assert_eq!(
                highlights(&result).0[0].1.font_style,
                Some(FontStyle::Italic)
            );

            let TextEdit {
                range, replacement, ..
            } = format_selection(&result, selected, Format::Italic).unwrap();
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
            let TextEdit {
                range,
                replacement,
                selection_after: selected,
            } = format_selection(text, selection.clone(), format).unwrap();
            let mut result = text.to_owned();
            result.replace_range(range, &replacement);
            assert_eq!(result, expected);
            assert_eq!(&result[selected], "Café");
        }
    }

    #[test]
    fn formats_multiple_paragraphs_and_keeps_whitespace_outside_delimiters() {
        let text = " first  \n\nsecond ";
        let TextEdit {
            range,
            replacement,
            selection_after: selected,
        } = format_selection(text, 0..text.len(), Format::Strike).unwrap();
        let mut result = text.to_owned();
        result.replace_range(range, &replacement);
        assert_eq!(result, " ~~first~~  \n\n~~second~~ ");

        let TextEdit {
            range, replacement, ..
        } = format_selection(&result, selected, Format::Strike).unwrap();
        result.replace_range(range, &replacement);
        assert_eq!(result, text);
    }

    #[test]
    fn preserves_the_content_selection_when_toggling_bold_inline() {
        let text = "é beforeCafé after";
        let start = text.find("Café").unwrap();
        let TextEdit {
            range,
            replacement,
            selection_after: selected,
        } = format_selection(text, start..start + "Café".len(), Format::Bold).unwrap();
        let mut result = text.to_owned();
        result.replace_range(range, &replacement);
        assert_eq!(result, "é before**Café** after");
        assert_eq!(&result[selected.clone()], "Café");

        let TextEdit {
            range,
            replacement,
            selection_after: selected,
        } = format_selection(&result, selected, Format::Bold).unwrap();
        result.replace_range(range, &replacement);
        assert_eq!(result, text);
        assert_eq!(&result[selected], "Café");
    }

    #[test]
    fn formats_inline_content_and_preserves_block_markers() {
        let text = concat!(
            "# Café #\n\n",
            "- one\n",
            "  1. two\n",
            "- [x] completed\n\n",
            "[ ] todo\n[X] done\n\n",
            "> quoted\n",
            "> continued\n\n",
            "Setext\n======",
        );
        let expected = concat!(
            "# **Café** #\n\n",
            "- **one**\n",
            "  1. **two**\n",
            "- [x] **completed**\n\n",
            "[ ] **todo**\n[X] **done**\n\n",
            "> **quoted**\n",
            "> **continued**\n\n",
            "**Setext**\n======",
        );
        let TextEdit {
            range,
            replacement,
            selection_after: selected,
        } = format_selection(text, 0..text.len(), Format::Bold).unwrap();
        let mut result = text.to_owned();
        result.replace_range(range, &replacement);
        assert_eq!(result, expected);

        let TextEdit {
            range, replacement, ..
        } = format_selection(&result, selected, Format::Bold).unwrap();
        result.replace_range(range, &replacement);
        assert_eq!(result, text);
    }

    #[test]
    fn applies_bold_to_only_unformatted_parts_of_a_mixed_selection() {
        let text = "**First**\nSecond";
        for start in [0, 2] {
            let TextEdit {
                range,
                replacement,
                selection_after: selected,
            } = format_selection(text, start..text.len(), Format::Bold).unwrap();
            let mut result = text.to_owned();
            result.replace_range(range, &replacement);
            assert_eq!(result, "**First**\n**Second**");

            let TextEdit {
                range,
                replacement,
                selection_after: selected,
            } = format_selection(&result, selected, Format::Bold).unwrap();
            result.replace_range(range, &replacement);
            assert_eq!(result, "First\nSecond");
            assert_eq!(&result[selected], result);
        }
    }

    #[test]
    fn styles_markdown_but_skips_escaped_markers_and_code() {
        let text = r"é ***Café*** ~~gone~~ `**code**` \*literal\*";
        let (styles, _) = highlights(text);
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

    #[test]
    fn detects_web_links_and_skips_code_and_other_schemes() {
        let text = concat!(
            "Café https://example.com, [notes](https://example.org/page), ",
            "<https://example.net>, [reference][docs], ",
            "`https://example.io` and [app](custom:action).\n\n",
            "[docs]: https://example.edu/guide",
        );
        let (_, links) = highlights(text);
        let found: Vec<_> = links
            .iter()
            .map(|link| (&text[link.range.clone()], link.url.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("https://example.com", "https://example.com/"),
                (
                    "[notes](https://example.org/page)",
                    "https://example.org/page"
                ),
                ("<https://example.net>", "https://example.net/"),
                ("[reference][docs]", "https://example.edu/guide"),
            ]
        );
    }

    #[test]
    fn detects_bare_domains_in_prose_without_replacing_markdown_destinations() {
        let text = concat!(
            "teobale.dev fsdfsf\nsfsdf www.teobale.dev sdfsdf\n",
            "Café (docs.example.org/guide), [teobale.dev](https://example.com/), ",
            "`teobale.dev`, me@teobale.dev, and 1.2.3.",
        );
        let (_, links) = highlights(text);
        let found: Vec<_> = links
            .iter()
            .map(|link| (&text[link.range.clone()], link.url.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("teobale.dev", "https://teobale.dev/"),
                ("www.teobale.dev", "http://www.teobale.dev/"),
                ("docs.example.org/guide", "https://docs.example.org/guide"),
                (
                    "[teobale.dev](https://example.com/)",
                    "https://example.com/"
                ),
            ]
        );
    }
}

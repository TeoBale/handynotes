use std::ops::Range;

use gpui_kit::component::input::{Rope, RopeExt};
use markdown::mdast::Node;

use crate::markup;
use crate::note_edit::{TextEdit, line_ending};

const MAX_LIST_NUMBER: u32 = 999_999_999;

#[derive(Default)]
pub(crate) struct FencedCode(Vec<Range<usize>>);

impl FencedCode {
    pub(crate) fn parse(text: &Rope) -> Self {
        if !text.bytes().any(|byte| matches!(byte, b'`' | b'~')) {
            return Self::default();
        }
        let text = text.to_string();
        if !text.contains("```") && !text.contains("~~~") {
            return Self::default();
        }
        markup::parse(&text)
            .map(|root| Self::from_root(&root, &text))
            .unwrap_or_default()
    }

    pub(crate) fn from_root(root: &Node, text: &str) -> Self {
        let mut ranges = Vec::new();
        for node in markup::nodes(root) {
            if let Node::Code(code) = node
                && let Some(position) = &code.position
            {
                let range = position.start.offset..position.end.offset;
                // The AST uses the same node type for indented and fenced code.
                let source = &text[range.clone()];
                if source.starts_with("```") || source.starts_with("~~~") {
                    ranges.push(range);
                }
            }
        }
        Self(ranges)
    }

    pub(crate) fn contains(&self, offset: usize) -> bool {
        let index = self.0.partition_point(|range| range.end <= offset);
        self.0
            .get(index)
            .is_some_and(|range| range.contains(&offset))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ListAction {
    Continue,
    RemoveEmpty,
    ToggleCheckbox,
}

pub(crate) fn list_edit(text: &Rope, cursor: usize, action: ListAction) -> Option<TextEdit> {
    let row = text.offset_to_point(cursor).row;
    let line_start = text.line_start_offset(row);
    let item = scan_line(text, row)?;
    let fenced = FencedCode::parse(text);
    if fenced.contains(item.marker.start) {
        return None;
    }
    let indent = text.slice(line_start..item.marker.start);

    let (mut range, mut replacement, caret, next_number) = match action {
        ListAction::ToggleCheckbox => {
            let ListKind::Checkbox(completed) = item.kind else {
                return None;
            };
            let mark = item.marker.start + 1;
            let replacement = if completed { " " } else { "x" };
            return Some(TextEdit {
                range: mark..mark + 1,
                replacement: replacement.into(),
                selection_after: cursor..cursor,
            });
        }
        ListAction::Continue if cursor == item.line_end && item.label_start.is_some() => {
            let newline = line_ending(text, row);
            let (marker, next_number) = match item.kind {
                ListKind::Checkbox(_) => ("[ ]".to_owned(), None),
                ListKind::Bullet => ("-".to_owned(), None),
                ListKind::Number(MAX_LIST_NUMBER) => return None,
                ListKind::Number(number) => (format!("{}.", number + 1), Some(number + 2)),
            };
            let replacement = format!("{newline}{indent}{marker} ");
            let caret = cursor + replacement.len();
            (cursor..cursor, replacement, caret, next_number)
        }
        ListAction::Continue | ListAction::RemoveEmpty
            if item.label_start.is_none() && cursor == item.line_end =>
        {
            let next_number = match item.kind {
                ListKind::Number(number) => Some(number),
                _ => None,
            };
            (
                item.marker.start..item.line_end,
                String::new(),
                item.marker.start,
                next_number,
            )
        }
        _ => return None,
    };
    if let Some(number) = next_number {
        range.end = renumber_following_items(
            text,
            &fenced,
            row,
            indentation_width(indent.bytes()),
            number,
            range.end,
            &mut replacement,
        );
    }
    Some(TextEdit {
        range,
        replacement,
        selection_after: caret..caret,
    })
}

fn indentation_width(bytes: impl Iterator<Item = u8>) -> usize {
    bytes
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .fold(0, |column, byte| {
            if byte == b'\t' {
                column + (4 - column % 4)
            } else {
                column + 1
            }
        })
}

fn renumber_following_items(
    text: &Rope,
    fenced: &FencedCode,
    row: usize,
    indentation: usize,
    mut number: u32,
    mut end: usize,
    replacement: &mut String,
) -> usize {
    for row in row + 1..text.lines_len() {
        let line = text.slice_line(row);
        if line
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r'))
        {
            break;
        }
        let width = indentation_width(line.bytes());
        if width > indentation {
            continue;
        }
        if width < indentation || number > MAX_LIST_NUMBER {
            break;
        }
        let Some(item) = list_line(text, row, fenced) else {
            break;
        };
        let ListKind::Number(current) = item.kind else {
            break;
        };
        if current != number {
            replacement.push_str(&text.slice(end..item.marker.start).to_string());
            replacement.push_str(&format!("{number}."));
            end = item.marker.end;
        }
        number += 1;
    }
    end
}

pub(crate) struct ListLine {
    pub(crate) marker: Range<usize>,
    pub(crate) kind: ListKind,
    pub(crate) label_start: Option<usize>,
    line_end: usize,
}

pub(crate) enum ListKind {
    Checkbox(bool),
    Bullet,
    Number(u32),
}

pub(crate) fn list_line(text: &Rope, row: usize, fenced: &FencedCode) -> Option<ListLine> {
    scan_line(text, row).filter(|item| !fenced.contains(item.marker.start))
}

pub(crate) fn scan_line(text: &Rope, row: usize) -> Option<ListLine> {
    let line = text.slice_line(row);
    let mut bytes = line
        .bytes()
        .enumerate()
        .skip_while(|(_, byte)| matches!(byte, b' ' | b'\t'))
        .peekable();
    let (marker_start, first) = bytes.next()?;
    let line_start = text.line_start_offset(row);
    let (marker_end, kind) = match first {
        b'[' => {
            let completed = match bytes.next()?.1 {
                b' ' => false,
                b'x' | b'X' => true,
                _ => return None,
            };
            if bytes.next()?.1 != b']' {
                return None;
            }
            (marker_start + 3, ListKind::Checkbox(completed))
        }
        b'-' => (marker_start + 1, ListKind::Bullet),
        b'0'..=b'9' => {
            let mut number = u32::from(first - b'0');
            while let Some((index, digit)) = bytes.next_if(|(_, byte)| byte.is_ascii_digit()) {
                if index - marker_start >= 9 {
                    return None;
                }
                number = number * 10 + u32::from(digit - b'0');
            }
            let (period, b'.') = bytes.next()? else {
                return None;
            };
            (period + 1, ListKind::Number(number))
        }
        _ => return None,
    };
    if !matches!(bytes.peek(), None | Some((_, b' ' | b'\t' | b'\r'))) {
        return None;
    }
    let line_end = line.len() - usize::from(line.byte(line.len() - 1) == b'\r');
    let label_start = bytes
        .find(|(_, byte)| !matches!(byte, b' ' | b'\t' | b'\r'))
        .map(|(offset, _)| line_start + offset);
    Some(ListLine {
        marker: line_start + marker_start..line_start + marker_end,
        kind,
        label_start,
        line_end: line_start + line_end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_in_the_middle_and_renumbers_only_following_siblings() {
        let original =
            "7. Ciao\r\n8. Miao\r\n9. Bau\r\n   1. Nested\r\n10. More\r\n\r\n1. Separate";
        let mut note = Rope::from(original);
        let cursor = original.find("8. Miao").unwrap() + "8. Miao".len();
        let TextEdit {
            range,
            replacement,
            selection_after,
        } = list_edit(&note, cursor, ListAction::Continue).unwrap();
        note.replace(range, &replacement);
        assert_eq!(
            note.to_string(),
            "7. Ciao\r\n8. Miao\r\n9. \r\n10. Bau\r\n   1. Nested\r\n11. More\r\n\r\n1. Separate"
        );
        let caret = cursor + "\r\n9. ".len();
        assert_eq!(selection_after, caret..caret);

        let TextEdit {
            range, replacement, ..
        } = list_edit(&note, caret, ListAction::RemoveEmpty).unwrap();
        note.replace(range, &replacement);
        assert_eq!(
            note.to_string(),
            "7. Ciao\r\n8. Miao\r\n\r\n9. Bau\r\n   1. Nested\r\n10. More\r\n\r\n1. Separate"
        );
    }

    #[test]
    fn continues_bulleted_and_numbered_lists_and_exits_empty_items() {
        for (original, continued, cleared) in [
            ("  - Café", "  - Café\n  - ", "  - Café\n  "),
            ("\t9. Café", "\t9. Café\n\t10. ", "\t9. Café\n\t"),
        ] {
            let mut note = Rope::from(original);
            let TextEdit {
                range, replacement, ..
            } = list_edit(&note, note.len(), ListAction::Continue).unwrap();
            note.replace(range, &replacement);
            assert_eq!(note.to_string(), continued);

            for action in [ListAction::Continue, ListAction::RemoveEmpty] {
                let TextEdit {
                    range, replacement, ..
                } = list_edit(&note, note.len(), action).unwrap();
                let mut result = note.clone();
                result.replace(range, &replacement);
                assert_eq!(result.to_string(), cleared);
            }

            assert!(list_edit(&note, original.len(), ListAction::ToggleCheckbox).is_none());
        }
    }

    #[test]
    fn only_continues_list_markers_at_the_start_of_a_line() {
        for plain in ["See 1. detail", "1.example", "---", "-word", "12.5"] {
            let note = Rope::from(plain);
            assert!(list_edit(&note, note.len(), ListAction::Continue).is_none());
        }
        let note = Rope::from("2. Two\r\nnext");
        assert_eq!(
            list_edit(&note, 6, ListAction::Continue),
            Some(TextEdit {
                range: 6..6,
                replacement: "\r\n3. ".into(),
                selection_after: 11..11
            })
        );
        assert!(list_edit(&note, 4, ListAction::Continue).is_none());
    }

    #[test]
    fn inherits_the_previous_line_ending_at_eof() {
        for (original, expected) in [
            ("1. One\r\n2. Two", "\r\n3. "),
            ("1. One\r\n2. Two\n3. Three", "\n4. "),
        ] {
            let note = Rope::from(original);
            let cursor = note.len();
            assert_eq!(
                list_edit(&note, cursor, ListAction::Continue),
                Some(TextEdit {
                    range: cursor..cursor,
                    replacement: expected.into(),
                    selection_after: cursor + expected.len()..cursor + expected.len()
                })
            );
        }
    }

    #[test]
    fn skips_list_markers_in_fenced_code() {
        let original = "  ````markdown\r\n[x] Fenced\r\n[ ]\r\n```\r\n1. Example\r\n````\r\n[ ] Outside\r\n\r\n~~~\r\n- Unclosed";
        let note = Rope::from(original);
        let fenced = FencedCode::parse(&note);
        for row in [1, 2, 4, 9] {
            assert!(list_line(&note, row, &fenced).is_none());
        }
        for (line, action) in [
            ("[x] Fenced", ListAction::ToggleCheckbox),
            ("[ ]\r\n", ListAction::RemoveEmpty),
            ("1. Example", ListAction::Continue),
            ("- Unclosed", ListAction::Continue),
        ] {
            let cursor = original.find(line).unwrap() + line.trim_end().len();
            assert!(list_edit(&note, cursor, action).is_none());
        }
        let cursor = original.find("[ ] Outside").unwrap();
        assert!(list_edit(&note, cursor, ListAction::ToggleCheckbox).is_some());
    }

    #[test]
    fn preserves_fenced_numbers_during_renumbering() {
        let original = "1. First\n   ```\n   3. Fenced\n   ```\n2. Second";
        let mut note = Rope::from(original);
        let TextEdit {
            range, replacement, ..
        } = list_edit(&note, "1. First".len(), ListAction::Continue).unwrap();
        note.replace(range, &replacement);
        assert_eq!(
            note.to_string(),
            "1. First\n2. \n   ```\n   3. Fenced\n   ```\n3. Second"
        );
    }

    #[test]
    fn continues_checkboxes_and_clears_empty_markers() {
        let mut note = Rope::from("\t[x] Café");
        let TextEdit {
            range, replacement, ..
        } = list_edit(&note, note.len(), ListAction::Continue).unwrap();
        note.replace(range, &replacement);
        assert_eq!(note.to_string(), "\t[x] Café\n\t[ ] ");

        for action in [ListAction::Continue, ListAction::RemoveEmpty] {
            let TextEdit {
                range, replacement, ..
            } = list_edit(&note, note.len(), action).unwrap();
            let mut cleared = note.clone();
            cleared.replace(range, &replacement);
            assert_eq!(cleared.to_string(), "\t[x] Café\n\t");
        }

        assert!(list_edit(&note, 5, ListAction::Continue).is_none());
        assert!(list_edit(&note, "\t[x] Café".len(), ListAction::RemoveEmpty).is_none());
        let plain = Rope::from("See [ ] example");
        assert!(list_edit(&plain, plain.len(), ListAction::Continue).is_none());
    }

    #[test]
    fn toggles_plain_text_checkboxes() {
        let mut note = Rope::from("// note\n[ ] Café /x\n[ ] Next");
        let cursor = "// note\n[ ] Café /x".len();
        for expected in [
            "// note\n[x] Café /x\n[ ] Next",
            "// note\n[ ] Café /x\n[ ] Next",
        ] {
            let TextEdit {
                range, replacement, ..
            } = list_edit(&note, cursor, ListAction::ToggleCheckbox).unwrap();
            note.replace(range, &replacement);
            assert_eq!(note.to_string(), expected);
        }
    }
}

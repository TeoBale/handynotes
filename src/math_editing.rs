use std::{collections::VecDeque, ops::Range};

use gpui_kit::component::input::{InputEdit, Rope, RopeExt};

use crate::note_edit::TextEdit;
use crate::note_math::{ResultSpan, math_results};

const MAX_CORRECTIONS: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryDirection {
    Undo,
    Redo,
}

struct Correction {
    uncorrected: Rope,
    corrected: Rope,
    selection_after: Range<usize>,
}

pub(crate) struct MathEditing {
    text: Rope,
    spans: Vec<ResultSpan>,
    corrections: VecDeque<Correction>,
    replaying: bool,
}

impl MathEditing {
    pub(crate) fn new(text: Rope) -> Self {
        let spans = math_results(&text, &[])
            .into_iter()
            .map(|(span, _)| span)
            .collect();
        Self {
            text,
            spans,
            corrections: VecDeque::new(),
            replaying: false,
        }
    }

    pub(crate) fn blocks_typing(&self, selection: &Range<usize>, delimiter: bool) -> bool {
        self.spans.iter().any(|span| {
            (selection.start > span.range.start
                || ((delimiter || !selection.is_empty()) && selection.start == span.range.start))
                && selection.end <= span.line_end
        })
    }

    pub(crate) fn is_replaying(&self) -> bool {
        self.replaying
    }

    pub(crate) fn begin_replay(&mut self) -> bool {
        if self.replaying || self.corrections.is_empty() {
            return false;
        }
        self.replaying = true;
        true
    }

    pub(crate) fn history_companion(
        &self,
        before: &Rope,
        after: &Rope,
        direction: HistoryDirection,
    ) -> Option<Range<usize>> {
        if before == after {
            return None;
        }
        self.corrections
            .iter()
            .rev()
            .find(|change| match direction {
                HistoryDirection::Undo => {
                    change.corrected == *before && change.uncorrected == *after
                }
                HistoryDirection::Redo => change.uncorrected == *after,
            })
            .map(|change| change.selection_after.clone())
    }

    pub(crate) fn finish_replay(&mut self, text: Rope) {
        self.replaying = false;
        self.spans = math_results(&text, &[])
            .into_iter()
            .map(|(span, _)| span)
            .collect();
        self.text = text;
    }

    pub(crate) fn reconcile(
        &mut self,
        raw: &Rope,
        edits: &[(InputEdit, Rope)],
        selection: Range<usize>,
    ) -> Option<TextEdit> {
        if self.replaying
            || self.text == *raw
            || edits.last().is_none_or(|(_, snapshot)| snapshot != raw)
        {
            return None;
        }

        let incoming = raw.to_string();
        if self.spans.is_empty() && !incoming.contains("math:") {
            self.text = raw.clone();
            return None;
        }
        let before = self.text.to_string();
        let mut upper_bound = before.len();
        let mut changes = Vec::with_capacity(edits.len());
        // Native batches normally run from the highest offset down. IME
        // commits can report the whole document, so recover the changed range.
        for (edit, snapshot) in edits {
            if edit.old_end_byte > upper_bound
                || (edit.start_byte == 0 && edit.old_end_byte == before.len())
            {
                changes.clear();
                break;
            }
            upper_bound = edit.start_byte;
            changes.push((
                edit.start_byte..edit.old_end_byte,
                snapshot
                    .slice(edit.start_byte..edit.new_end_byte)
                    .to_string(),
            ));
        }
        if changes.is_empty() {
            let (range, replacement) = changed_text(&before, &incoming)?;
            changes.push((range, replacement.to_owned()));
        }

        let (mut accepted, markers) = apply_user_changes(&before, changes, &self.spans);
        let mut selection_after = selection.clone();
        if let Some((range, replacement)) = changed_text(&incoming, &accepted) {
            move_selection(&mut selection_after, &range, replacement.len());
        }

        let results = math_results(&Rope::from(accepted.as_str()), &markers);
        let mut shift = 0;
        self.spans = results
            .iter()
            .map(|(span, replacement)| {
                let start = span.range.start.saturating_add_signed(shift);
                let width = replacement.as_ref().map_or(span.range.len(), String::len);
                let equals = replacement
                    .as_ref()
                    .map_or(span.equals.saturating_add_signed(shift), |_| start + 1);
                shift += width as isize - span.range.len() as isize;
                ResultSpan {
                    range: start..start + width,
                    equals,
                    line_end: span.line_end.saturating_add_signed(shift),
                }
            })
            .collect();
        for (span, replacement) in results.into_iter().rev() {
            if let Some(replacement) = replacement {
                move_selection(&mut selection_after, &span.range, replacement.len());
                accepted.replace_range(span.range, &replacement);
            }
        }

        let change = changed_text(&incoming, &accepted);
        let mut final_text = raw.clone();
        if let Some((range, replacement)) = &change {
            final_text.replace(range.clone(), replacement);
            if self.corrections.len() == MAX_CORRECTIONS {
                self.corrections.pop_front();
            }
            self.corrections.push_back(Correction {
                uncorrected: raw.clone(),
                corrected: final_text.clone(),
                selection_after: selection_after.clone(),
            });
        }
        self.text = final_text;
        change.map(|(range, replacement)| TextEdit {
            range,
            replacement: replacement.to_owned(),
            selection_after,
        })
    }
}

fn apply_user_changes(
    before: &str,
    changes: Vec<(Range<usize>, String)>,
    spans: &[ResultSpan],
) -> (String, Vec<usize>) {
    let mut accepted = before.to_owned();
    let mut markers: Vec<_> = spans.iter().map(|span| span.equals).collect();
    let mut applied_start = before.len();
    for (range, text) in changes {
        let Some((mut range, text)) = protect(before, range, &text, spans) else {
            continue;
        };
        if range.end > applied_start {
            if text.is_empty() && range.start < applied_start {
                range.end = applied_start;
            } else {
                continue;
            }
        }
        markers.retain_mut(|marker| {
            if range.contains(marker) {
                return false;
            }
            if *marker >= range.end {
                *marker = *marker - range.len() + text.len();
            }
            true
        });
        applied_start = range.start;
        accepted.replace_range(range, text);
    }
    (accepted, markers)
}

fn protect<'a>(
    before: &str,
    mut range: Range<usize>,
    text: &'a str,
    spans: &[ResultSpan],
) -> Option<(Range<usize>, &'a str)> {
    let first = spans.partition_point(|span| span.line_end < range.start);
    for span in &spans[first..] {
        if span.range.start > range.end {
            break;
        }
        if range.is_empty() && range.start > span.range.start {
            return text
                .starts_with(['\r', '\n'])
                .then_some((span.line_end..span.line_end, text));
        }
        if !range.is_empty()
            && range.start >= span.range.start
            && range.end <= span.line_end
            && !text.is_empty()
        {
            return None;
        }
        if range.start < span.range.end && range.end > span.range.start {
            range.start = range.start.min(span.range.start);
            range.end = range.end.max(span.range.end);
        } else if range.start >= span.range.end
            && range.end > span.line_end
            && !before[range.end..].is_empty()
            && !before[range.end..].starts_with(['\r', '\n'])
            && !text.contains(['\r', '\n'])
        {
            return None;
        }
    }
    Some((range, text))
}

fn changed_text<'a>(before: &str, after: &'a str) -> Option<(Range<usize>, &'a str)> {
    let start: usize = before
        .chars()
        .zip(after.chars())
        .take_while(|(old, new)| old == new)
        .map(|(ch, _)| ch.len_utf8())
        .sum();
    if start == before.len() && start == after.len() {
        return None;
    }
    let tail: usize = before[start..]
        .chars()
        .rev()
        .zip(after[start..].chars().rev())
        .take_while(|(old, new)| old == new)
        .map(|(ch, _)| ch.len_utf8())
        .sum();
    Some((
        start..before.len() - tail,
        &after[start..after.len() - tail],
    ))
}

fn move_selection(selection: &mut Range<usize>, range: &Range<usize>, inserted: usize) {
    for offset in [&mut selection.start, &mut selection.end] {
        if *offset >= range.end {
            *offset = *offset - range.len() + inserted;
        } else if *offset > range.start {
            *offset = range.start + inserted;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        math: &mut MathEditing,
        range: Range<usize>,
        replacement: &str,
    ) -> (Rope, Range<usize>) {
        let before = math.text.clone();
        let mut after = before.clone();
        after.replace(range.clone(), replacement);
        let new_end = range.start + replacement.len();
        let edit = InputEdit {
            start_byte: range.start,
            old_end_byte: range.end,
            new_end_byte: new_end,
            start_position: before.offset_to_point(range.start),
            old_end_position: before.offset_to_point(range.end),
            new_end_position: after.offset_to_point(new_end),
        };
        let mut selection = new_end..new_end;
        if let Some(edit) = math.reconcile(&after, &[(edit, after.clone())], selection.clone()) {
            after.replace(edit.range, &edit.replacement);
            selection = edit.selection_after;
        }
        assert_eq!(math.text, after);
        (after, selection)
    }

    #[test]
    fn recomputes_assigned_values_and_later_results_in_the_same_block() {
        let source = "math:\nn: 5 * 2 = 10\nn + 3 = 13\n;\n5 + 3 = 8\nmath:\n1 + 2 = 3";
        let mut math = MathEditing::new(Rope::from(source));
        let offset = source.find("5 *").unwrap();
        let (text, selection) = input(&mut math, offset..offset + 1, "25");
        assert_eq!(
            text.to_string(),
            "math:\nn: 25 * 2 = 50\nn + 3 = 53\n;\n5 + 3 = 8\nmath:\n1 + 2 = 3"
        );
        assert_eq!(selection, offset + 2..offset + 2);
    }

    #[test]
    fn keeps_activation_when_expression_is_temporarily_invalid() {
        let source = "math:\n5 + 3 = 8";
        let mut math = MathEditing::new(Rope::from(source));
        let offset = source.find('3').unwrap();
        let (text, selection) = input(&mut math, offset..offset + 1, "");
        assert_eq!(text.to_string(), "math:\n5 + = n/a");
        let caret = selection.start.min("math:\n5 +".len());
        let (text, _) = input(&mut math, caret..caret, " 4");
        assert_eq!(text.to_string(), "math:\n5 + 4 = 9");
    }

    #[test]
    fn adds_unavailable_results_for_pasted_calculations_only_inside_math_blocks() {
        let mut math = MathEditing::new(Rope::default());
        let (text, _) = input(
            &mut math,
            0..0,
            "math:\n12 usd in eur * 3 =\nFollow up =\n;\n1 / 0 =",
        );
        assert_eq!(
            text.to_string(),
            "math:\n12 usd in eur * 3 = n/a\nFollow up =\n;\n1 / 0 ="
        );
    }

    #[test]
    fn deletes_the_whole_result_from_any_position_including_currency_symbols() {
        let source = "math:\n5 eur * 2 = 10€\nplain text";
        for part in ["=", "1", "0", "€"] {
            let mut math = MathEditing::new(Rope::from(source));
            let offset = source.find(part).unwrap();
            let (text, selection) = input(&mut math, offset..offset + part.len(), "");
            assert_eq!(text.to_string(), "math:\n5 eur * 2\nplain text");
            let end = "math:\n5 eur * 2".len();
            assert_eq!(selection, end..end);
        }
    }

    #[test]
    fn rejects_typing_over_a_result_and_pasting_after_it() {
        let source = "math:\n5 + 3 = 8  // legacy comment\nplain text";
        let mut math = MathEditing::new(Rope::from(source));
        let result = source.find('8').unwrap();
        let end = source.find("\nplain").unwrap();
        assert!(math.blocks_typing(&(result..result), false));
        assert!(math.blocks_typing(&(end..end), false));
        let (text, _) = input(&mut math, result..result + 1, "9");
        assert_eq!(text.to_string(), source);
        let (text, _) = input(&mut math, end..end, " appended\nsecond line");
        assert_eq!(text.to_string(), source);
        let (text, _) = input(&mut math, end..end + 1, "");
        assert_eq!(text.to_string(), source);
        let (text, _) = input(&mut math, result..result, "\n");
        assert_eq!(text.to_string(), source.replace("\nplain", "\n\nplain"));
    }

    #[test]
    fn identifies_the_extra_native_history_step_for_recalculation() {
        let original = Rope::from("math:\n5 + 3 = 8");
        let mut math = MathEditing::new(original.clone());
        let offset = "math:\n".len();
        let (corrected, selection) = input(&mut math, offset..offset + 1, "6");
        let raw = Rope::from("math:\n6 + 3 = 8");
        assert_eq!(
            math.history_companion(&corrected, &raw, HistoryDirection::Undo),
            Some(selection.clone())
        );
        assert_eq!(
            math.history_companion(&original, &raw, HistoryDirection::Redo),
            Some(selection)
        );
        assert_eq!(
            math.history_companion(&original, &corrected, HistoryDirection::Redo),
            None
        );
    }
}

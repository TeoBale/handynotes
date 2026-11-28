use std::ops::Range;

use gpui_kit::component::input::{EditorState, Rope, RopeExt};
use gpui_kit::{Context, EntityInputHandler, Window};

pub(crate) fn line_ending(text: &Rope, row: usize) -> &'static str {
    let Some(newline) = text.line_start_offset(row + 1).checked_sub(1) else {
        return "\n";
    };
    let newline = if text.byte(newline) == b'\n' {
        newline
    } else if row > 0 {
        text.line_start_offset(row) - 1
    } else {
        return "\n";
    };
    if newline > 0 && text.byte(newline - 1) == b'\r' {
        "\r\n"
    } else {
        "\n"
    }
}

pub(crate) fn comment_offset(bytes: impl Iterator<Item = u8>) -> Option<usize> {
    let mut bytes = bytes.enumerate().peekable();
    // A whitespace boundary allows trailing comments without matching slashes in URLs.
    let mut at_boundary = true;
    while let Some((offset, byte)) = bytes.next() {
        if at_boundary && byte == b'/' && matches!(bytes.peek(), Some((_, b'/'))) {
            return Some(offset);
        }
        at_boundary = matches!(byte, b' ' | b'\t');
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TextEdit {
    pub(crate) range: Range<usize>,
    pub(crate) replacement: String,
    pub(crate) selection_after: Range<usize>,
}

impl TextEdit {
    pub(crate) fn apply_atomic(
        self,
        state: &mut EditorState,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        state.set_selected_range(self.range, cx);
        state.replace(self.replacement, window, cx);
        state.set_selected_range(self.selection_after, cx);
    }

    pub(crate) fn apply(
        self,
        state: &mut EditorState,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        if !state.is_editable() {
            return;
        }

        let before = state.selected_range();
        let reversed = state.is_selection_reversed();
        let old_text = state.text().slice(self.range.clone());
        let changed = !old_text.bytes().eq(self.replacement.bytes());
        let cursor = if reversed { before.start } else { before.end };
        let insert_at_cursor = self.range.is_empty() && self.range.start == cursor;
        // GPUI can group an explicit insertion or a one-character rewrite with typing.
        let may_coalesce = changed
            && !insert_at_cursor
            && !self.replacement.contains(['\r', '\n'])
            && (self.range.is_empty()
                || (before.is_empty()
                    && before.end == self.range.end
                    && old_text.chars().count() == 1
                    && !old_text.chars().any(|ch| matches!(ch, '\r' | '\n'))));

        if changed {
            if state.marked_text_range(window, cx).is_some() {
                state.unmark_text(window, cx);
            }
            if insert_at_cursor {
                state.insert(self.replacement, window, cx);
            } else {
                if may_coalesce {
                    // Reapplying the real selection separates this command from prior typing.
                    state.set_selected_range_with_direction(before, reversed, cx);
                }
                let text = state.text();
                let utf16_range = text.offset_to_offset_utf16(self.range.start)
                    ..text.offset_to_offset_utf16(self.range.end);
                // The explicit range makes GPUI record the actual selection for Undo.
                state.replace_text_in_range(Some(utf16_range), &self.replacement, window, cx);
            }
        }

        if may_coalesce
            || state.selected_range() != self.selection_after
            || state.is_selection_reversed() != (reversed && !self.selection_after.is_empty())
        {
            state.set_selected_range_with_direction(self.selection_after, reversed, cx);
        }
    }
}

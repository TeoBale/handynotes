use std::{borrow::Cow, ops::Range, rc::Rc, time::Duration};

use gpui_kit::base::Scrollbar;
use gpui_kit::component::input::{
    Editor, EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, Rope,
    RopeExt,
};
use gpui_kit::{
    Animation, AnimationExt, App, AppContext as _, Bounds, Context, Entity, FontWeight,
    HighlightStyle, IntoElement, ParentElement, Pixels, SharedString, Styled, Window, canvas, div,
    ease_out_quint, fill, point, px, rgb, size,
};

const PAPER: u32 = 0xffffff;
const DOT: u32 = 0xe1eaf3;
const COMMENT: u32 = 0x525252;
const COMMENT_PREFIX: &str = "// ";
const HEADING_COLORS: [u32; 6] = [
    0x89566c, // Dusty rose
    0x8b5e48, // Clay
    0x77652e, // Ochre
    0x557043, // Sage
    0x366e78, // Teal
    0x675985, // Lavender
];

pub const INK: u32 = 0x000000;
pub const NOTE_FONT: &str = "Inter";
pub const NOTE_FONT_SIZE: Pixels = px(15.);

pub fn register_fonts(cx: &App) {
    cx.text_system()
        .add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/inter/Inter-Regular.ttf")),
            Cow::Borrowed(include_bytes!("../assets/fonts/inter/Inter-SemiBold.ttf")),
        ])
        .expect("load bundled Inter fonts");
}

pub fn editor_state(initial: &str, window: &mut Window, cx: &mut App) -> Entity<EditorState> {
    cx.new(|cx| {
        let mut state = EditorState::new(window, cx)
            .default_value(initial.to_owned())
            .language("handynotes-markdown")
            .placeholder("Start writing…")
            .soft_wrap(true)
            .line_number(false)
            .indent_guides(false)
            .folding(false)
            .auto_close(false)
            .smart_indent(false)
            .searchable(false);
        state.set_scrollbar_renderer(Some(Rc::new(|viewport, _, _| {
            Scrollbar::vertical(&viewport)
                .styles(|styles| {
                    styles
                        .thumb(|thumb| thumb.width(px(3.)).inset(px(0.)))
                        .thumb_hover(|thumb| thumb.width(px(4.)).inset(px(0.)))
                        .thumb_active(|thumb| thumb.width(px(4.)).inset(px(0.)))
                })
                .into_any_element()
        })));
        state.set_highlighter_factory(Rc::new(|_| Some(Box::<NoteHighlighter>::default())), cx);
        state
    })
}

pub fn toggle_comment(state: &mut EditorState, window: &mut Window, cx: &mut Context<EditorState>) {
    let selection = state.selected_range();
    let reversed = state.is_selection_reversed();
    if let Some((range, replacement, selection)) = comment_edit(state.text(), selection) {
        state.set_selected_range(range, cx);
        state.replace(replacement, window, cx);
        state.set_selected_range_with_direction(selection, reversed, cx);
    }
}

fn comment_edit(
    text: &Rope,
    selection: Range<usize>,
) -> Option<(Range<usize>, String, Range<usize>)> {
    let first = text.offset_to_point(selection.start).row;
    // A selection ending at the start of a line doesn't include that line.
    let last_byte = selection.end - usize::from(!selection.is_empty());
    let last = text.offset_to_point(last_byte).row;
    let range = text.line_start_offset(first)..text.line_end_offset(last);
    let source = text.slice(range.clone()).to_string();
    let lines = source
        .split('\n')
        .map(|line| (line, line.trim_start_matches([' ', '\t'])));
    let skip_empty = |content: &str| first != last && matches!(content, "" | "\r");
    let uncomment = lines
        .clone()
        .all(|(_, content)| skip_empty(content) || content.starts_with("//"));

    let mut replacement = String::with_capacity(source.len());
    let mut adjusted = selection.clone();
    let mut line_start = range.start;
    for (index, (line, content)) in lines.enumerate() {
        if index != 0 {
            replacement.push('\n');
        }
        let indent = line.len() - content.len();
        let (removed, added) = if skip_empty(content) {
            (0, "")
        } else if uncomment {
            let marker_len = if content.starts_with(COMMENT_PREFIX) {
                COMMENT_PREFIX.len()
            } else {
                "//".len()
            };
            (marker_len, "")
        } else {
            (0, COMMENT_PREFIX)
        };
        replacement.push_str(&line[..indent]);
        replacement.push_str(added);
        replacement.push_str(&content[removed..]);

        let marker_start = line_start + indent;
        for (old, new) in [
            (selection.start, &mut adjusted.start),
            (selection.end, &mut adjusted.end),
        ] {
            if old >= marker_start {
                *new += added.len();
                *new -= removed.min(old - marker_start);
            }
        }
        line_start += line.len() + 1;
    }

    (replacement != source).then_some((range, replacement, adjusted))
}

pub fn paper(state: &Entity<EditorState>, direction: Option<isize>, cx: &App) -> impl IntoElement {
    // Account for the editor's built-in line leading and horizontal inset.
    // Consume the extra first-line space as the document scrolls.
    let first_line_space = (px(15.) + state.read(cx).scroll_offset().y).clamp(px(0.), px(15.));
    let editor = div().size_full().pt(first_line_space).pl(px(17.)).child(
        Editor::new(state)
            .aria_label("Note")
            .appearance(false)
            .h(gpui_kit::relative(1.0))
            .font_family(NOTE_FONT)
            .text_color(rgb(INK))
            .text_size(NOTE_FONT_SIZE)
            .line_height(px(23.)),
    );
    let editor = match direction {
        Some(direction) => editor
            .with_animation(
                ("note-slide", state.entity_id()),
                Animation::new(Duration::from_millis(210)).with_easing(ease_out_quint()),
                move |editor, progress| {
                    editor
                        .left(px(96. * direction as f32 * (1. - progress)))
                        .opacity(0.78 + 0.22 * progress)
                },
            )
            .into_any_element(),
        None => editor.into_any_element(),
    };
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_hidden()
        .bg(rgb(PAPER))
        .child(
            canvas(
                |_, _, _| (),
                |area, (), window, _| {
                    let spacing = px(16.);
                    let origin = area.origin;
                    let mut y = px(12.);
                    while y < area.size.height {
                        let mut x = px(12.);
                        while x < area.size.width {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(origin.x + x, origin.y + y),
                                    size(px(1.), px(1.)),
                                ),
                                rgb(DOT),
                            ));
                            x += spacing;
                        }
                        y += spacing;
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(editor)
}

#[derive(Default)]
struct NoteHighlighter(Rope);

impl InputHighlighter for NoteHighlighter {
    fn language(&self) -> SharedString {
        "handynotes-markdown".into()
    }

    fn update(
        &mut self,
        _: Option<InputEdit>,
        text: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        self.0 = text.clone();
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let text = &self.0;
        if range.is_empty() || range.end > text.len() {
            return vec![(range.clone(), HighlightStyle::default())];
        }

        let first_line = text.offset_to_point(range.start).row;
        let last_line = text.offset_to_point(range.end - 1).row;
        let mut runs = Vec::with_capacity(last_line - first_line + 1);

        for row in first_line..=last_line {
            let line = text.slice_line(row);
            let line_start = text.line_start_offset(row);
            let start = line_start.max(range.start);
            let end = text.line_start_offset(row + 1).min(range.end);
            let comment_start = comment_offset(line.bytes())
                .map_or(end, |offset| (line_start + offset).clamp(start, end));

            if start < comment_start {
                let style = match heading_level(line.chars()) {
                    Some(level) => HighlightStyle {
                        color: Some(rgb(HEADING_COLORS[level - 1]).into()),
                        font_weight: Some(FontWeight::SEMIBOLD),
                        ..HighlightStyle::default()
                    },
                    None => HighlightStyle::default(),
                };
                runs.push((start..comment_start, style));
            }
            if comment_start < end {
                runs.push((
                    comment_start..end,
                    HighlightStyle {
                        color: Some(rgb(COMMENT).into()),
                        ..HighlightStyle::default()
                    },
                ));
            }
        }

        runs
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

fn heading_level(chars: impl Iterator<Item = char>) -> Option<usize> {
    let mut chars = chars.peekable();
    for _ in 0..3 {
        if chars.next_if_eq(&' ').is_none() {
            break;
        }
    }

    let mut markers = 0;
    while chars.next_if_eq(&'#').is_some() {
        markers += 1;
    }
    ((1..=6).contains(&markers) && matches!(chars.next(), None | Some(' ' | '\t' | '\r')))
        .then_some(markers)
}

fn comment_offset(bytes: impl Iterator<Item = u8>) -> Option<usize> {
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

#[cfg(test)]
mod tests {
    use gpui_kit::base::input::InputEditorStyle;

    use super::*;

    #[test]
    fn toggles_selected_lines_and_preserves_selection_and_indentation() {
        let original = "  Café\r\n\r\n\t// detail\r\nnext";
        let selection = 4..original.find("next").unwrap();
        let mut note = original.to_owned();

        let (range, replacement, adjusted) =
            comment_edit(&Rope::from(note.as_str()), selection.clone()).unwrap();
        note.replace_range(range, &replacement);
        assert_eq!(note, "  // Café\r\n\r\n\t// // detail\r\nnext");
        assert_eq!(&note[adjusted.clone()], "fé\r\n\r\n\t// // detail\r\n");

        let (range, replacement, restored) =
            comment_edit(&Rope::from(note.as_str()), adjusted).unwrap();
        note.replace_range(range, &replacement);
        assert_eq!(note, original);
        assert_eq!(restored, selection);
    }

    #[test]
    fn toggles_at_the_caret_and_removes_markers_without_spaces() {
        let (range, replacement, selection) = comment_edit(&Rope::from("a\n"), 2..2).unwrap();
        assert_eq!(range, 2..2);
        assert_eq!(replacement, "// ");
        assert_eq!(selection, 5..5);

        let (range, replacement, selection) =
            comment_edit(&Rope::from("  //Café\n"), 4..4).unwrap();
        assert_eq!(range, 0..9);
        assert_eq!(replacement, "  Café");
        assert_eq!(selection, 2..2);
    }

    #[test]
    fn styles_headings_and_comments_around_unicode_and_links() {
        let note = concat!(
            "# Café // later\n",
            "\t// comment\n",
            "## Clay\n",
            "### Ochre\n",
            "#### Sage\n",
            "##### Teal\n",
            "###### Lavender\n",
            "See https://example.com\n",
            "#tag",
        );
        let highlighter = NoteHighlighter(Rope::from(note));
        let resolver = InputEditorStyle::default().highlight_styles;
        let runs: Vec<_> = highlighter
            .styles(&(0..note.len()), resolver.as_ref())
            .into_iter()
            .map(|(range, style)| (&note[range], style.color))
            .collect();

        assert_eq!(
            runs,
            [
                ("# Café ", Some(rgb(HEADING_COLORS[0]).into())),
                ("// later\n", Some(rgb(COMMENT).into())),
                ("\t", None),
                ("// comment\n", Some(rgb(COMMENT).into())),
                ("## Clay\n", Some(rgb(HEADING_COLORS[1]).into())),
                ("### Ochre\n", Some(rgb(HEADING_COLORS[2]).into())),
                ("#### Sage\n", Some(rgb(HEADING_COLORS[3]).into())),
                ("##### Teal\n", Some(rgb(HEADING_COLORS[4]).into())),
                ("###### Lavender\n", Some(rgb(HEADING_COLORS[5]).into())),
                ("See https://example.com\n", None),
                ("#tag", None),
            ]
        );

        let clipped = note.find("later").unwrap()..note.find("\n").unwrap();
        let runs = highlighter.styles(&clipped, resolver.as_ref());
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, clipped);
        assert_eq!(runs[0].1.color, Some(rgb(COMMENT).into()));
    }
}

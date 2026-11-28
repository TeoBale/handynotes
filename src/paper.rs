use std::{borrow::Cow, cell::RefCell, ops::Range, rc::Rc, time::Duration};

use gpui_kit::base::input::{AutoClosingPair, Backspace, Enter, set_language_config};
use gpui_kit::component::input::language_config::LanguageConfig;
use gpui_kit::component::input::{
    DisplayPoint, Editor, EditorState, FoldRange, HighlightStyleResolver, InputEdit,
    InputHighlighter, Rope, RopeExt, TextDecoration, TextDecorationCollection,
};
use gpui_kit::component::{Sizable, Size};
use gpui_kit::{
    Animation, AnimationExt, App, AppContext as _, Bounds, ClickEvent, Context, Entity,
    EntityInputHandler, FontWeight, HighlightStyle, HitboxBehavior, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Pixels, Point, SharedString,
    StatefulInteractiveElement, StrikethroughStyle, Styled, UnderlineStyle, Window, canvas,
    combine_highlights, div, ease_out_quint, fill, font, point, px, rgb, size,
};

use crate::list_edit::{FencedCode, ListAction, ListKind, list_edit, list_line, scan_line};
use crate::markup::{self, Format, WebLink};
use crate::math_editing::MathEditing;
use crate::note_edit::{TextEdit, comment_offset};
use crate::note_math::{MathTrigger, math_edit};
use crate::note_scroll::{NOTE_MARGIN, NoteScroll};

const NOTE_LANGUAGE: &str = "handynotes-markdown";
const PAIRS: [(&str, &str); 6] = [
    ("(", ")"),
    ("[", "]"),
    ("{", "}"),
    ("\"", "\""),
    ("'", "'"),
    ("`", "`"),
];
const PAPER: u32 = 0xffffff;
const DOT: u32 = 0xe1eaf3;
const COMMENT: u32 = 0x525252;
const LINK: u32 = 0x245a96;
const COMPLETED_BOX: u32 = 0x9b60bb;
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
            Cow::Borrowed(include_bytes!("../assets/fonts/inter/Inter-Italic.ttf")),
            Cow::Borrowed(include_bytes!(
                "../assets/fonts/inter/Inter-SemiBoldItalic.ttf"
            )),
            Cow::Borrowed(include_bytes!("../assets/fonts/inter/Inter-Bold.ttf")),
            Cow::Borrowed(include_bytes!("../assets/fonts/inter/Inter-BoldItalic.ttf")),
        ])
        .expect("load bundled Inter fonts");
}

pub fn register_language(cx: &mut App) {
    let pairs = PAIRS.map(|(open, close)| AutoClosingPair::new(open, close));
    set_language_config(
        NOTE_LANGUAGE,
        LanguageConfig::default().auto_closing_pairs(pairs),
        cx,
    );
}

#[derive(Clone)]
pub struct NoteEditor {
    pub state: Entity<EditorState>,
    pub(crate) math: Rc<RefCell<MathEditing>>,
    scroll: NoteScroll,
    links: Rc<RefCell<Vec<WebLink>>>,
    link_hover: TextDecorationCollection,
}

pub fn editor_state(initial: &str, window: &mut Window, cx: &mut App) -> NoteEditor {
    let scroll = NoteScroll::new();
    let scrollbar = scroll.clone();
    let links = Rc::new(RefCell::new(Vec::new()));
    let highlight_links = links.clone();
    let state = cx.new(|cx| {
        let mut state = EditorState::new(window, cx)
            .default_value(initial.to_owned())
            .language(NOTE_LANGUAGE)
            .placeholder("Start writing…")
            .soft_wrap(true)
            .line_number(false)
            .indent_guides(false)
            .folding(false)
            .auto_close(true)
            .smart_indent(false)
            // Leave room for the caret when the first-row inset shifts the editor down.
            .cursor_surrounding_lines(Some(2))
            .searchable(false);
        state.set_scrollbar_renderer(Some(Rc::new(move |viewport, window, _| {
            scrollbar.scrollbar(viewport, window)
        })));
        state.set_highlighter_factory(
            Rc::new(move |_| {
                Some(Box::new(NoteHighlighter::new(
                    Rope::default(),
                    highlight_links.clone(),
                )))
            }),
            cx,
        );
        state
    });
    let link_hover = state.update(cx, |state, cx| {
        state.create_decorations_collection(Vec::new(), cx)
    });
    NoteEditor {
        state,
        math: Rc::new(RefCell::new(MathEditing::new(Rope::from(initial)))),
        scroll,
        links,
        link_hover,
    }
}

pub fn toggle_comment(state: &mut EditorState, window: &mut Window, cx: &mut Context<EditorState>) {
    if let Some(edit) = comment_edit(state.text(), state.selected_range()) {
        edit.apply(state, window, cx);
    }
}

fn comment_edit(text: &Rope, selection: Range<usize>) -> Option<TextEdit> {
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

    (replacement != source).then_some(TextEdit {
        range,
        replacement,
        selection_after: adjusted,
    })
}

pub fn paper(note: &NoteEditor, direction: Option<isize>, cx: &App) -> impl IntoElement {
    let state = &note.state;
    let scroll = note.scroll.clone();
    let top_inset = scroll.top_inset(state.read(cx));
    let scroll_state = state.clone();
    let enter_state = state.clone();
    let backspace_state = state.clone();
    let click_state = state.clone();
    let key_state = state.clone();
    let math = note.math.clone();
    let click_links = note.links.clone();
    let hover_links = note.links.clone();
    let hover_state = state.clone();
    let link_hover = note.link_hover.clone();
    let clear_hover = note.link_hover.clone();
    let editor = div()
        .id(("note-editor", state.entity_id()))
        .size_full()
        // Move the viewport without resizing it, which would trigger caret reveal.
        .relative()
        .top(top_inset)
        .pl(NOTE_MARGIN - Size::XSmall.input_px())
        .pr(px(6.))
        .capture_action(move |action: &Enter, window, cx| {
            if !action.shift {
                if !action.secondary && apply_math(&enter_state, MathTrigger::Return, window, cx) {
                    cx.stop_propagation();
                    return;
                }
                let action = if action.secondary {
                    ListAction::ToggleCheckbox
                } else {
                    ListAction::Continue
                };
                handle_list_action(&enter_state, action, window, cx);
            }
        })
        .capture_action(move |_: &Backspace, window, cx| {
            handle_list_action(&backspace_state, ListAction::RemoveEmpty, window, cx);
        })
        .on_click(move |event, window, cx| {
            if let Some(link) =
                clicked_link(click_state.read(cx), &click_links.borrow(), event, window)
            {
                cx.open_url(&link.url);
            } else {
                toggle_clicked_checkbox(&click_state, event, window, cx);
            }
        })
        .on_mouse_move(move |event, window, cx| {
            let links = hover_links.borrow();
            let hovered = link_at_position(hover_state.read(cx), &links, event.position, window);
            link_hover.set(
                hovered
                    .into_iter()
                    .map(|link| {
                        TextDecoration::new(
                            link.range.clone(),
                            HighlightStyle {
                                underline: Some(UnderlineStyle {
                                    thickness: px(1.),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
                cx,
            );
        })
        .on_hover(move |hovered, _, cx| {
            if !hovered {
                clear_hover.clear(cx);
            }
        })
        .capture_key_down(move |event, window, cx| {
            let stroke = &event.keystroke;
            if !stroke.modifiers.platform
                && !stroke.modifiers.control
                && stroke
                    .key_char
                    .as_deref()
                    .is_some_and(|key| key.chars().all(|ch| !ch.is_control()))
                && math.borrow().blocks_typing(
                    &key_state.read(cx).selected_range(),
                    stroke.key_char.as_deref() == Some("="),
                )
            {
                window.prevent_default();
                cx.stop_propagation();
                return;
            }
            if stroke.key_char.as_deref() == Some("=")
                && !stroke.modifiers.platform
                && !stroke.modifiers.control
                && apply_math(&key_state, MathTrigger::Equals, window, cx)
            {
                window.prevent_default();
                cx.stop_propagation();
                return;
            }
            edit_selected_text(&key_state, event, window, cx)
        })
        .child(
            Editor::new(state)
                .aria_label("Note")
                .appearance(false)
                .with_size(Size::XSmall)
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
                |area, window, _| window.insert_hitbox(area, HitboxBehavior::Normal),
                move |area, hitbox, window, _| {
                    scroll.listen(hitbox, scroll_state, window);
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

fn apply_math(
    editor: &Entity<EditorState>,
    trigger: MathTrigger,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let state = editor.read(cx);
    if !state.is_editable() {
        return false;
    }
    let Some(edit) = math_edit(state.text(), state.selected_range(), trigger) else {
        return false;
    };
    editor.update(cx, |state, cx| {
        if state.marked_text_range(window, cx).is_some() {
            return false;
        }
        edit.apply(state, window, cx);
        true
    })
}

fn clicked_link<'a>(
    state: &EditorState,
    links: &'a [WebLink],
    event: &ClickEvent,
    window: &Window,
) -> Option<&'a WebLink> {
    let ClickEvent::Mouse(click) = event else {
        return None;
    };
    let command_only = |modifiers: gpui_kit::Modifiers| {
        modifiers.platform && !modifiers.shift && !modifiers.alt && !modifiers.control
    };
    if click.down.click_count != 1
        || !command_only(click.down.modifiers)
        || !command_only(click.up.modifiers)
        || !state.selected_range().is_empty()
    {
        return None;
    }
    let link = link_at_position(state, links, click.up.position, window)?;
    (link_at_position(state, links, click.down.position, window) == Some(link)).then_some(link)
}

fn link_at_position<'a>(
    state: &EditorState,
    links: &'a [WebLink],
    pointer: Point<Pixels>,
    window: &Window,
) -> Option<&'a WebLink> {
    if links.is_empty() || !state.input_bounds().contains(&pointer) {
        return None;
    }
    let y = pointer.y - state.text_bounds()?.top();
    let line_height = state.line_height()?;
    let map = state.display_map();
    let row = (y / line_height) as usize;
    if y < px(0.) || row >= map.display_row_count() {
        return None;
    }
    let text = state.text();
    let start = map.display_pos_to_buffer_pos(DisplayPoint::new(row, 0));
    if !state.visible_row_range()?.contains(&start.line) {
        return None;
    }
    let row_start = text.line_start_offset(start.line) + start.col;
    let mut row_end = text.line_end_offset(start.line);
    if row + 1 < map.display_row_count() {
        let next = map.display_pos_to_buffer_pos(DisplayPoint::new(row + 1, 0));
        row_end = row_end.min(text.line_start_offset(next.line) + next.col);
    }
    let first = links.partition_point(|link| link.range.end <= row_start);
    links[first..]
        .iter()
        .take_while(|link| link.range.start < row_end)
        .find(|link| {
            let range = link.range.start.max(row_start)..link.range.end.min(row_end);
            // At a soft wrap, the end caret is on the next row. Measure the
            // final glyph on this row to avoid treating the margin as a link.
            let right = state.range_to_bounds(&range).and_then(|bounds| {
                if bounds.size.height == line_height {
                    return Some(bounds);
                }
                let ch = text.chars_at(range.end).reversed().next()?;
                let last = range.end - ch.len_utf8();
                let mut bounds = state.range_to_bounds(&(range.start..last))?;
                let system = window.text_system();
                let font_id = system.resolve_font(&font(NOTE_FONT));
                bounds.size.width += system.layout_width(font_id, NOTE_FONT_SIZE, ch);
                Some(bounds)
            });
            right.is_some_and(|bounds| bounds.contains(&pointer))
        })
}

fn edit_selected_text(
    editor: &Entity<EditorState>,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let state = editor.read(cx);
    let selection = state.selected_range();
    let stroke = &event.keystroke;
    let modifiers = stroke.modifiers;
    if selection.is_empty() || modifiers.control {
        return;
    }

    let edit = if modifiers.platform && !modifiers.alt && !modifiers.shift {
        let Some(format) = Format::for_key(&stroke.key) else {
            return;
        };
        (!event.is_held)
            .then(|| markup::format_selection(&state.value(), selection, format))
            .flatten()
    } else if !modifiers.platform {
        let Some((open, close)) = PAIRS
            .into_iter()
            .find(|(open, close)| matches!(stroke.key_char.as_deref(), Some(key) if key == *open || key == *close))
        else {
            return;
        };
        (!event.is_held).then(|| {
            let selected = state.text().slice(selection.clone());
            let wrapped = format!("{open}{selected}{close}");
            let inner = selection.start + open.len()..selection.end + open.len();
            TextEdit {
                range: selection,
                replacement: wrapped,
                selection_after: inner,
            }
        })
    } else {
        return;
    };

    if let Some(edit) = edit {
        editor.update(cx, |state, cx| {
            edit.apply(state, window, cx);
        });
    }
    window.prevent_default();
    cx.stop_propagation();
}

fn handle_list_action(
    editor: &Entity<EditorState>,
    action: ListAction,
    window: &mut Window,
    cx: &mut App,
) {
    let selection = editor.read(cx).selected_range();
    if selection.is_empty() {
        apply_list_action(editor, selection.start, action, window, cx);
    }
}

fn toggle_clicked_checkbox(
    editor: &Entity<EditorState>,
    event: &ClickEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let ClickEvent::Mouse(click) = event else {
        return;
    };
    let state = editor.read(cx);
    let viewport = state.input_bounds();
    if click.down.click_count != 1
        || click.down.modifiers.modified()
        || click.up.modifiers.modified()
        || !state.selected_range().is_empty()
        || !viewport.contains(&click.down.position)
        || !viewport.contains(&click.up.position)
    {
        return;
    }
    let marker = state.visible_row_range().and_then(|mut rows| {
        rows.find_map(|row| {
            // The edit validates the candidate against fenced code.
            let item = scan_line(state.text(), row)?;
            if !matches!(item.kind, ListKind::Checkbox(_)) {
                return None;
            }
            let bounds = state.range_to_bounds(&item.marker)?;
            (Some(bounds.size.height) == state.line_height()
                && bounds.contains(&click.down.position)
                && bounds.contains(&click.up.position))
            .then_some(item.marker.start)
        })
    });
    if let Some(offset) = marker {
        apply_list_action(editor, offset, ListAction::ToggleCheckbox, window, cx);
    }
}

fn apply_list_action(
    editor: &Entity<EditorState>,
    offset: usize,
    action: ListAction,
    window: &mut Window,
    cx: &mut App,
) {
    let state = editor.read(cx);
    let Some(mut edit) = list_edit(state.text(), offset, action) else {
        return;
    };
    if matches!(action, ListAction::ToggleCheckbox) {
        edit.selection_after = state.selected_range();
    }
    editor.update(cx, |state, cx| {
        edit.apply(state, window, cx);
    });
    cx.stop_propagation();
}

#[derive(Default)]
struct NoteHighlighter {
    text: Rope,
    inline: Vec<(Range<usize>, HighlightStyle)>,
    fenced: FencedCode,
    links: Rc<RefCell<Vec<WebLink>>>,
}

impl NoteHighlighter {
    fn new(text: Rope, links: Rc<RefCell<Vec<WebLink>>>) -> Self {
        let (inline, found, fenced) = if text
            .bytes()
            .any(|byte| matches!(byte, b'*' | b'_' | b'~' | b':' | b'.' | b'`'))
        {
            let source = text.to_string();
            markup::parse(&source).map_or_else(Default::default, |root| {
                let (inline, links) = markup::highlights(&source, &root);
                (inline, links, FencedCode::from_root(&root, &source))
            })
        } else {
            Default::default()
        };
        let link_styles = found.iter().map(|link| {
            (
                link.range.clone(),
                HighlightStyle {
                    color: Some(rgb(LINK).into()),
                    ..Default::default()
                },
            )
        });
        let inline = combine_highlights(inline, link_styles).collect();
        links.replace(found);
        Self {
            text,
            inline,
            fenced,
            links,
        }
    }

    fn apply_inline_styles(
        &self,
        runs: Vec<(Range<usize>, HighlightStyle)>,
        range: &Range<usize>,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let first = self
            .inline
            .partition_point(|(span, _)| span.end <= range.start);
        let mut inline = self.inline[first..].iter().peekable();
        if inline
            .peek()
            .is_none_or(|(span, _)| span.start >= range.end)
        {
            return runs;
        }

        let mut result = Vec::with_capacity(runs.len());
        for (base, style) in runs {
            let mut start = base.start;
            while let Some((span, overlay)) = inline.peek() {
                if span.start >= base.end {
                    break;
                }
                let overlap_start = start.max(span.start);
                let overlap_end = base.end.min(span.end);
                if start < overlap_start {
                    result.push((start..overlap_start, style));
                }
                if overlap_start < overlap_end {
                    let mut combined = style.highlight(*overlay);
                    // A completed checkbox already supplies its strike color.
                    combined.strikethrough = style.strikethrough.or(overlay.strikethrough);
                    result.push((overlap_start..overlap_end, combined));
                    start = overlap_end;
                }
                if span.end > base.end {
                    break;
                }
                inline.next();
            }
            if start < base.end {
                result.push((start..base.end, style));
            }
        }
        result
    }
}

impl InputHighlighter for NoteHighlighter {
    fn language(&self) -> SharedString {
        NOTE_LANGUAGE.into()
    }

    fn update(
        &mut self,
        _: Option<InputEdit>,
        text: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        *self = Self::new(text.clone(), self.links.clone());
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let text = &self.text;
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
            let row_end = text.line_start_offset(row + 1);
            let end = row_end.min(range.end);
            let muted = HighlightStyle {
                color: Some(rgb(COMMENT).into()),
                ..HighlightStyle::default()
            };
            if let Some(checkbox) = list_line(text, row, &self.fenced)
                && matches!(checkbox.kind, ListKind::Checkbox(true))
            {
                let label_start = checkbox.label_start.unwrap_or(row_end);
                let plain = HighlightStyle::default();
                let lilac = HighlightStyle {
                    color: Some(rgb(COMPLETED_BOX).into()),
                    ..plain
                };
                let struck = HighlightStyle {
                    strikethrough: Some(StrikethroughStyle {
                        thickness: px(1.),
                        color: muted.color,
                    }),
                    ..muted
                };
                for (span, style) in [
                    (line_start..checkbox.marker.start, plain),
                    (checkbox.marker.clone(), lilac),
                    (checkbox.marker.end..label_start, plain),
                    (label_start..row_end, struck),
                ] {
                    let span = span.start.max(start)..span.end.min(end);
                    if !span.is_empty() {
                        runs.push((span, style));
                    }
                }
                continue;
            }
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
                runs.push((comment_start..end, muted));
            }
        }

        self.apply_inline_styles(runs, range)
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

#[cfg(test)]
mod tests {
    use gpui_kit::base::input::InputEditorStyle;

    use super::*;

    #[test]
    fn strikes_only_completed_labels_outside_fenced_code() {
        let note = Rope::from("// note\n[x] Café /x\n[ ] Next\n```\n[x] Example\n```");
        let highlighter = NoteHighlighter::new(note.clone(), Rc::default());
        let resolver = InputEditorStyle::default().highlight_styles;
        let runs = highlighter.styles(&(0..note.len()), resolver.as_ref());
        assert_eq!(runs[1].1.color, Some(rgb(COMPLETED_BOX).into()));
        assert!(runs[1].1.strikethrough.is_none());
        assert_eq!(runs[0].1.color, runs[3].1.color);
        assert_eq!(note.slice(runs[3].0.clone()).to_string(), "Café /x\n");
        assert!(runs[3].1.strikethrough.is_some());
        assert!(
            runs[4..]
                .iter()
                .all(|(_, style)| *style == HighlightStyle::default())
        );
    }

    #[test]
    fn toggles_selected_lines_and_preserves_selection_and_indentation() {
        let original = "  Café\r\n\r\n\t// detail\r\nnext";
        let selection = 4..original.find("next").unwrap();
        let mut note = original.to_owned();

        let TextEdit {
            range,
            replacement,
            selection_after: adjusted,
        } = comment_edit(&Rope::from(note.as_str()), selection.clone()).unwrap();
        note.replace_range(range, &replacement);
        assert_eq!(note, "  // Café\r\n\r\n\t// // detail\r\nnext");
        assert_eq!(&note[adjusted.clone()], "fé\r\n\r\n\t// // detail\r\n");

        let TextEdit {
            range,
            replacement,
            selection_after: restored,
        } = comment_edit(&Rope::from(note.as_str()), adjusted).unwrap();
        note.replace_range(range, &replacement);
        assert_eq!(note, original);
        assert_eq!(restored, selection);
    }

    #[test]
    fn toggles_at_the_caret_and_removes_markers_without_spaces() {
        let TextEdit {
            range,
            replacement,
            selection_after: selection,
        } = comment_edit(&Rope::from("a\n"), 2..2).unwrap();
        assert_eq!(range, 2..2);
        assert_eq!(replacement, "// ");
        assert_eq!(selection, 5..5);

        let TextEdit {
            range,
            replacement,
            selection_after: selection,
        } = comment_edit(&Rope::from("  //Café\n"), 4..4).unwrap();
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
        let highlighter = NoteHighlighter::new(Rope::from(note), Rc::default());
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
                ("See ", None),
                ("https://example.com", Some(rgb(LINK).into())),
                ("\n", None),
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

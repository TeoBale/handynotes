use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
    time::Duration,
};

use gpui_kit::base::input::{
    AutoClosingPair, Backspace, EditorScrollbarContext, Enter, set_language_config,
};
use gpui_kit::base::{Scrollbar, ScrollbarHandle};
use gpui_kit::component::input::language_config::LanguageConfig;
use gpui_kit::component::input::{
    DisplayPoint, Editor, EditorState, FoldRange, HighlightStyleResolver, InputEdit,
    InputHighlighter, Rope, RopeExt, TextDecoration, TextDecorationCollection,
};
use gpui_kit::component::{Sizable, Size};
use gpui_kit::{
    Animation, AnimationExt, App, AppContext as _, Bounds, ClickEvent, Context, Entity, FontWeight,
    HighlightStyle, Hitbox, HitboxBehavior, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Pixels, PlatformInput, Point, ScrollDelta, ScrollEvent, SharedString,
    StatefulInteractiveElement, StrikethroughStyle, Styled, UnderlineStyle, Window, canvas,
    combine_highlights, deferred, div, ease_out_quint, fill, font, point, px, rgb, size,
};

use crate::markup::{self, Format, WebLink};

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
const NOTE_MARGIN: Pixels = px(23.);
const SCROLL_SPEED: f32 = 0.65;
const SCROLL_EPSILON: Pixels = px(0.001);
const COMMENT: u32 = 0x525252;
const LINK: u32 = 0x245a96;
const MAX_LIST_NUMBER: u32 = 999_999_999;
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
    inset: Rc<Cell<Pixels>>,
    links: Rc<RefCell<Vec<WebLink>>>,
    link_hover: TextDecorationCollection,
}

#[derive(Clone)]
struct NoteScrollbar {
    editor: EditorScrollbarContext,
    inset: Rc<Cell<Pixels>>,
    track: Bounds<Pixels>,
}

impl ScrollbarHandle for NoteScrollbar {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.track
    }

    fn content_size(&self) -> gpui_kit::Size<Pixels> {
        let mut content = self.editor.content_size();
        if content.height > self.track.size.height {
            content.height += NOTE_MARGIN;
        }
        content
    }

    fn offset(&self) -> Point<Pixels> {
        let mut offset = self.editor.offset();
        offset.y = note_scroll_offset(offset.y, visible_inset(offset.y, &self.inset));
        offset
    }

    fn set_offset(&self, mut offset: Point<Pixels>) {
        // The track includes the inset before the first row, so its thumb
        // continues toward the top while the editor itself is at offset zero.
        self.inset.set(inset_at_scroll_offset(offset.y));
        offset.y = (offset.y + NOTE_MARGIN).min(px(0.));
        self.editor.set_offset(offset);
    }
}

fn note_scroll_offset(editor_offset: Pixels, inset: Pixels) -> Pixels {
    editor_offset + inset - NOTE_MARGIN
}

fn inset_at_scroll_offset(offset: Pixels) -> Pixels {
    (offset + NOTE_MARGIN).clamp(px(0.), NOTE_MARGIN)
}

fn visible_inset(offset: Pixels, inset: &Cell<Pixels>) -> Pixels {
    if offset < -SCROLL_EPSILON {
        px(0.)
    } else {
        inset.get()
    }
}

pub fn editor_state(initial: &str, window: &mut Window, cx: &mut App) -> NoteEditor {
    let inset = Rc::new(Cell::new(NOTE_MARGIN));
    let scrollbar_inset = inset.clone();
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
            let mut track = viewport.viewport_bounds();
            track.origin.y = window.visual_viewport_bounds().top();
            let handle = NoteScrollbar {
                editor: viewport,
                inset: scrollbar_inset.clone(),
                track,
            };
            let scrollbar = Scrollbar::vertical(&handle).styles(|styles| {
                styles
                    .thumb(|thumb| thumb.width(px(3.)).inset(px(0.)))
                    .thumb_hover(|thumb| thumb.width(px(4.)).inset(px(0.)))
                    .thumb_active(|thumb| thumb.width(px(4.)).inset(px(0.)))
            });
            // Draw outside the translated editor's clip with a window-fixed track.
            deferred(scrollbar).into_any_element()
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
        inset,
        links,
        link_hover,
    }
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

pub fn paper(note: &NoteEditor, direction: Option<isize>, cx: &App) -> impl IntoElement {
    let state = &note.state;
    let inset = note.inset.clone();
    let offset = state.read(cx).scroll_offset().y;
    if offset < -SCROLL_EPSILON {
        inset.set(NOTE_MARGIN);
    }
    let top_inset = visible_inset(offset, &inset);
    let scroll_state = state.clone();
    let enter_state = state.clone();
    let backspace_state = state.clone();
    let click_state = state.clone();
    let key_state = state.clone();
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
                    scroll_note(hitbox, scroll_state, inset, window);
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

fn scroll_note(
    hitbox: Hitbox,
    editor: Entity<EditorState>,
    inset: Rc<Cell<Pixels>>,
    window: &mut Window,
) {
    let replaying = Rc::new(Cell::new(false));
    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
        if !phase.capture() || replaying.get() || !hitbox.should_handle_scroll(window) {
            return;
        }
        let line_height = editor
            .read(cx)
            .line_height()
            .unwrap_or(window.line_height());
        let pixels = event.delta.pixel_delta(line_height);
        if pixels.y == px(0.) || pixels.y.abs() < pixels.x.abs() {
            return;
        }
        let mut event = event.clone();
        let editor = editor.clone();
        let inset = inset.clone();
        let replaying = replaying.clone();
        cx.stop_propagation();
        // GPUI restores its mouse listeners after dispatch. Replay then so its
        // editor still handles the gesture phases, momentum and edge behavior.
        window.defer(cx, move |window, cx| {
            let offset = editor.read(cx).scroll_offset().y;
            let old_inset = visible_inset(offset, &inset);
            let travel = pixels.y * SCROLL_SPEED;
            // Scroll through the blank inset before scrolling text into the clip.
            let destination = note_scroll_offset(offset, old_inset) + travel;
            let new_inset = inset_at_scroll_offset(destination);
            if new_inset != old_inset {
                inset.set(new_inset);
                window.refresh();
            }
            let remaining = travel - (new_inset - old_inset);
            let factor = if remaining.abs() < SCROLL_EPSILON {
                0.
            } else {
                remaining / pixels.y
            };
            event.delta = match event.delta {
                ScrollDelta::Pixels(delta) => {
                    ScrollDelta::Pixels(point(delta.x * factor, delta.y * factor))
                }
                ScrollDelta::Lines(delta) => {
                    ScrollDelta::Lines(point(delta.x * factor, delta.y * factor))
                }
            };
            replaying.set(true);
            window.dispatch_event(PlatformInput::Scroll(event), cx);
            replaying.set(false);
        });
    });
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
            (selection, wrapped, inner)
        })
    } else {
        return;
    };

    if let Some((range, replacement, selection)) = edit {
        let reversed = state.is_selection_reversed();
        editor.update(cx, |state, cx| {
            if state.selected_range() != range {
                state.set_selected_range(range, cx);
            }
            state.replace(replacement, window, cx);
            state.set_selected_range_with_direction(selection, reversed, cx);
        });
    }
    window.prevent_default();
    cx.stop_propagation();
}

#[derive(Clone, Copy)]
enum ListAction {
    Continue,
    RemoveEmpty,
    ToggleCheckbox,
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
            let item = list_line(state.text(), row)?;
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
    let Some((range, replacement, caret)) = list_edit(state.text(), offset, action) else {
        return;
    };
    let caret = if matches!(action, ListAction::ToggleCheckbox) {
        state.cursor()
    } else {
        caret
    };
    editor.update(cx, |state, cx| {
        state.set_selected_range(range, cx);
        state.replace(replacement, window, cx);
        if state.cursor() != caret {
            state.set_selected_range(caret..caret, cx);
        }
    });
    cx.stop_propagation();
}

fn list_edit(
    text: &Rope,
    cursor: usize,
    action: ListAction,
) -> Option<(Range<usize>, String, usize)> {
    let row = text.offset_to_point(cursor).row;
    let line_start = text.line_start_offset(row);
    let item = list_line(text, row)?;
    let indent = text.slice(line_start..item.marker.start);

    let (mut range, mut replacement, caret, next_number) = match action {
        ListAction::ToggleCheckbox => {
            let ListKind::Checkbox(completed) = item.kind else {
                return None;
            };
            let mark = item.marker.start + 1;
            let replacement = if completed { " " } else { "x" };
            return Some((mark..mark + 1, replacement.into(), cursor));
        }
        ListAction::Continue if cursor == item.line_end && item.label_start.is_some() => {
            let newline = if item.line_end < text.line_end_offset(row) {
                "\r\n"
            } else {
                "\n"
            };
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
            row,
            indentation_width(indent.bytes()),
            number,
            range.end,
            &mut replacement,
        );
    }
    Some((range, replacement, caret))
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
        let Some(item) = list_line(text, row) else {
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

struct ListLine {
    marker: Range<usize>,
    kind: ListKind,
    label_start: Option<usize>,
    line_end: usize,
}

enum ListKind {
    Checkbox(bool),
    Bullet,
    Number(u32),
}

fn list_line(text: &Rope, row: usize) -> Option<ListLine> {
    let line = text.slice_line(row);
    let mut bytes = line
        .bytes()
        .enumerate()
        .skip_while(|(_, byte)| matches!(byte, b' ' | b'\t'))
        .peekable();
    let (marker_start, first) = bytes.next()?;
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
    let line_start = text.line_start_offset(row);
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

#[derive(Default)]
struct NoteHighlighter {
    text: Rope,
    inline: Vec<(Range<usize>, HighlightStyle)>,
    links: Rc<RefCell<Vec<WebLink>>>,
}

impl NoteHighlighter {
    fn new(text: Rope, links: Rc<RefCell<Vec<WebLink>>>) -> Self {
        let (inline, found) = if text
            .bytes()
            .any(|byte| matches!(byte, b'*' | b'_' | b'~' | b':' | b'.'))
        {
            markup::highlights(&text.to_string())
        } else {
            (Vec::new(), Vec::new())
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
            if let Some(checkbox) = list_line(text, row)
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
    fn inserts_in_the_middle_and_renumbers_only_following_siblings() {
        let original =
            "7. Ciao\r\n8. Miao\r\n9. Bau\r\n   1. Nested\r\n10. More\r\n\r\n1. Separate";
        let mut note = Rope::from(original);
        let cursor = original.find("8. Miao").unwrap() + "8. Miao".len();
        let (range, replacement, caret) = list_edit(&note, cursor, ListAction::Continue).unwrap();
        note.replace(range, &replacement);
        assert_eq!(
            note.to_string(),
            "7. Ciao\r\n8. Miao\r\n9. \r\n10. Bau\r\n   1. Nested\r\n11. More\r\n\r\n1. Separate"
        );
        assert_eq!(caret, cursor + "\r\n9. ".len());

        let (range, replacement, _) = list_edit(&note, caret, ListAction::RemoveEmpty).unwrap();
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
            let (range, replacement, _) =
                list_edit(&note, note.len(), ListAction::Continue).unwrap();
            note.replace(range, &replacement);
            assert_eq!(note.to_string(), continued);

            for action in [ListAction::Continue, ListAction::RemoveEmpty] {
                let (range, replacement, _) = list_edit(&note, note.len(), action).unwrap();
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
            Some((6..6, "\r\n3. ".into(), 11))
        );
        assert!(list_edit(&note, 4, ListAction::Continue).is_none());
    }

    #[test]
    fn continues_checkboxes_and_clears_empty_markers() {
        let mut note = Rope::from("\t[x] Café");
        let (range, replacement, _) = list_edit(&note, note.len(), ListAction::Continue).unwrap();
        note.replace(range, &replacement);
        assert_eq!(note.to_string(), "\t[x] Café\n\t[ ] ");

        for action in [ListAction::Continue, ListAction::RemoveEmpty] {
            let (range, replacement, _) = list_edit(&note, note.len(), action).unwrap();
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
    fn toggles_plain_text_and_strikes_only_completed_labels() {
        let mut note = Rope::from("// note\n[ ] Café /x\n[ ] Next");
        let cursor = "// note\n[ ] Café /x".len();
        assert!(matches!(
            list_line(&note, 1).unwrap().kind,
            ListKind::Checkbox(false)
        ));
        let (range, replacement, _) = list_edit(&note, cursor, ListAction::ToggleCheckbox).unwrap();
        note.replace(range, &replacement);
        assert_eq!(note.to_string(), "// note\n[x] Café /x\n[ ] Next");

        let highlighter = NoteHighlighter::new(note.clone(), Rc::default());
        let resolver = InputEditorStyle::default().highlight_styles;
        let runs = highlighter.styles(&(0..note.len()), resolver.as_ref());
        assert_eq!(runs[1].1.color, Some(rgb(COMPLETED_BOX).into()));
        assert!(runs[1].1.strikethrough.is_none());
        assert_eq!(runs[0].1.color, runs[3].1.color);
        assert_eq!(note.slice(runs[3].0.clone()).to_string(), "Café /x\n");
        assert!(runs[3].1.strikethrough.is_some());
        assert!(runs[4].1.strikethrough.is_none());

        let (range, replacement, _) = list_edit(&note, cursor, ListAction::ToggleCheckbox).unwrap();
        note.replace(range, &replacement);
        assert_eq!(note.to_string(), "// note\n[ ] Café /x\n[ ] Next");
    }

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

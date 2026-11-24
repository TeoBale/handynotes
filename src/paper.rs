use std::{ops::Range, rc::Rc, time::Duration};

use gpui_kit::base::Scrollbar;
use gpui_kit::component::input::{
    Editor, EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, Rope,
    RopeExt,
};
use gpui_kit::{
    Animation, AnimationExt, App, AppContext as _, Bounds, Context, Entity, FontWeight,
    HighlightStyle, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, Window,
    canvas, div, ease_out_quint, fill, point, px, rgb, size,
};

const PAPER: u32 = 0xffffff;
const DOT: u32 = 0xc8d8e7;
const INK: u32 = 0x383633;
const PRIMARY_HEADING: u32 = 0x253b56;
const SECONDARY_HEADING: u32 = 0x277e85;

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
        state.set_highlighter_factory(Rc::new(|_| Some(Box::<MarkdownHeadings>::default())), cx);
        state
    })
}

pub fn paper(
    state: &Entity<EditorState>,
    direction: Option<isize>,
    controls: impl IntoElement,
) -> impl IntoElement {
    let editor = div().size_full().pt(px(15.)).pl(px(17.)).child(
        Editor::new(state)
            .aria_label("Note")
            .appearance(false)
            .h(gpui_kit::relative(1.0))
            .text_color(rgb(INK))
            .text_size(px(13.))
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
        .child(
            div()
                .group("footer")
                .absolute()
                .bottom_0()
                .left_0()
                .right(px(5.))
                .h(px(56.))
                .flex()
                .flex_col()
                .justify_end()
                .child(controls),
        )
}

#[derive(Default)]
struct MarkdownHeadings(Rope);

impl InputHighlighter for MarkdownHeadings {
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
            let start = text.line_start_offset(row).max(range.start);
            let end = text.line_start_offset(row + 1).min(range.end);
            let heading = heading_level(text.slice_line(row).chars());
            let style = heading.map_or_else(HighlightStyle::default, |level| HighlightStyle {
                color: Some(
                    rgb(if level == 1 {
                        PRIMARY_HEADING
                    } else {
                        SECONDARY_HEADING
                    })
                    .into(),
                ),
                font_weight: Some(FontWeight::SEMIBOLD),
                ..HighlightStyle::default()
            });
            runs.push((start..end, style));
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

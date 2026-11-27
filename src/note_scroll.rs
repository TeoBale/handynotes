use std::{cell::Cell, rc::Rc};

use gpui_kit::base::input::EditorScrollbarContext;
use gpui_kit::base::{Scrollbar, ScrollbarHandle};
use gpui_kit::component::input::EditorState;
use gpui_kit::{
    AnyElement, Bounds, Entity, Hitbox, IntoElement, Pixels, PlatformInput, Point, ScrollDelta,
    ScrollEvent, Size, Window, deferred, point, px,
};

pub(crate) const NOTE_MARGIN: Pixels = px(23.);
const SCROLL_SPEED: f32 = 0.65;
const SCROLL_EPSILON: Pixels = px(0.001);

// The scrollbar sees one continuous offset. GPUI sees a stationary viewport
// with its own offset, and we translate that viewport while the inset is visible.
#[derive(Clone, Copy)]
struct ScrollPosition {
    editor_offset: Pixels,
    inset: Pixels,
}

impl ScrollPosition {
    fn from_scrollbar(offset: Pixels) -> Self {
        let offset_with_margin = offset + NOTE_MARGIN;
        Self {
            editor_offset: offset_with_margin.min(px(0.)),
            inset: offset_with_margin.clamp(px(0.), NOTE_MARGIN),
        }
    }

    fn scrollbar_offset(self) -> Pixels {
        self.editor_offset + self.inset - NOTE_MARGIN
    }

    fn scroll_by(self, travel: Pixels) -> (Pixels, Pixels) {
        let destination = Self::from_scrollbar(self.scrollbar_offset() + travel);
        let remaining = travel - (destination.inset - self.inset);
        (destination.inset, remaining)
    }
}

#[derive(Clone)]
pub(crate) struct NoteScroll(Rc<Cell<Pixels>>);

impl NoteScroll {
    pub(crate) fn new() -> Self {
        Self(Rc::new(Cell::new(NOTE_MARGIN)))
    }

    fn position(&self, editor_offset: Pixels) -> ScrollPosition {
        let inset = if editor_offset < -SCROLL_EPSILON {
            px(0.)
        } else {
            self.0.get()
        };
        ScrollPosition {
            editor_offset,
            inset,
        }
    }

    pub(crate) fn top_inset(&self, editor: &EditorState) -> Pixels {
        if editor.text().len() == 0 {
            self.0.set(NOTE_MARGIN);
            return NOTE_MARGIN;
        }
        let offset = editor.scroll_offset().y;
        if offset < -SCROLL_EPSILON {
            // A caret or keyboard scroll can move the editor without going
            // through the inset. Restore the full inset when it returns to zero.
            self.0.set(NOTE_MARGIN);
        }
        self.position(offset).inset
    }

    pub(crate) fn scrollbar(&self, editor: EditorScrollbarContext, window: &Window) -> AnyElement {
        let mut track = editor.viewport_bounds();
        track.origin.y = window.visual_viewport_bounds().top();
        let handle = NoteScrollbar {
            editor,
            scroll: self.clone(),
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
    }

    pub(crate) fn listen(&self, hitbox: Hitbox, editor: Entity<EditorState>, window: &mut Window) {
        let scroll = self.clone();
        let replaying = Rc::new(Cell::new(false));
        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
            if !phase.capture() || replaying.get() || !hitbox.should_handle_scroll(window) {
                return;
            }
            let state = editor.read(cx);
            let line_height = state.line_height().unwrap_or(window.line_height());
            let pixels = event.delta.pixel_delta(line_height);
            if pixels.y == px(0.) || pixels.y.abs() < pixels.x.abs() {
                return;
            }
            let empty = state.text().len() == 0;
            cx.stop_propagation();
            if empty {
                return;
            }
            let mut event = event.clone();
            let editor = editor.clone();
            let scroll = scroll.clone();
            let replaying = replaying.clone();
            // GPUI restores its mouse listeners after dispatch. Replay then so
            // the editor still handles gesture phases, momentum, and overscroll.
            window.defer(cx, move |window, cx| {
                let position = scroll.position(editor.read(cx).scroll_offset().y);
                let (inset, remaining) = position.scroll_by(pixels.y * SCROLL_SPEED);
                if inset != position.inset {
                    scroll.0.set(inset);
                    window.refresh();
                }
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
}

#[derive(Clone)]
struct NoteScrollbar {
    editor: EditorScrollbarContext,
    scroll: NoteScroll,
    track: Bounds<Pixels>,
}

impl ScrollbarHandle for NoteScrollbar {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.track
    }

    fn content_size(&self) -> Size<Pixels> {
        let mut content = self.editor.content_size();
        if content.height > self.track.size.height {
            content.height += NOTE_MARGIN;
        }
        content
    }

    fn offset(&self) -> Point<Pixels> {
        let offset = self.editor.offset();
        point(offset.x, self.scroll.position(offset.y).scrollbar_offset())
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        let position = ScrollPosition::from_scrollbar(offset.y);
        self.scroll.0.set(position.inset);
        self.editor
            .set_offset(point(offset.x, position.editor_offset));
    }
}

#[cfg(test)]
mod tests {
    use super::{NOTE_MARGIN, ScrollPosition, px};

    #[test]
    fn scroll_uses_the_inset_before_moving_the_editor() {
        let top = ScrollPosition::from_scrollbar(px(0.));
        assert_eq!(top.scroll_by(px(-10.)), (NOTE_MARGIN - px(10.), px(0.)));
        assert_eq!(top.scroll_by(-NOTE_MARGIN - px(5.)), (px(0.), px(-5.)));
    }

    #[test]
    fn returning_to_the_top_exposes_the_inset_after_the_editor() {
        let scrolled = ScrollPosition::from_scrollbar(-NOTE_MARGIN - px(5.));
        assert_eq!(scrolled.scrollbar_offset(), -NOTE_MARGIN - px(5.));
        assert_eq!(scrolled.scroll_by(px(9.)), (px(4.), px(5.)));
    }
}

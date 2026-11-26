use chrono::{DateTime, Local};
use gpui_kit::{
    self as kit,
    component::{
        Theme, ThemeMode,
        input::{EditorState, Input, InputEvent, InputState},
    },
    prelude::*,
    *,
};
use store::NoteStore;

mod account;
mod cloud;
mod native;
mod oauth;
mod paper;
mod store;
mod sync;

struct DeletedNote {
    note: store::Note,
    undo_target: EntityId,
    target_text: SharedString,
}

struct Notebook {
    account: account::Account,
    editors: std::collections::HashMap<u64, Entity<EditorState>>,
    deletions: Vec<DeletedNote>,
    store: NoteStore,
    current: Option<u64>,
    editor: Entity<EditorState>,
    query: Entity<InputState>,
    editor_events: Option<Subscription>,
    searching: bool,
    slide_direction: Option<isize>,
    selected: usize,
    pending_delete: Option<u64>,
    scroll: ScrollHandle,
    error: Option<String>,
    shortcut_available: bool,
    toolbar_visible: bool,
}

impl Notebook {
    fn new(store: NoteStore, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search all notes"));
        cx.subscribe_in(&query, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.selected = 0;
                this.pending_delete = None;
                this.scroll.scroll_to_item(0);
                cx.notify();
            }
        })
        .detach();
        let editor = paper::editor_state("", window, cx);
        let (tx, rx) = async_channel::unbounded();
        let shortcut_available = native::install(window, tx);
        cx.spawn_in(window, async move |view, cx| {
            while let Ok(event) = rx.recv().await {
                if cx
                    .update(|window, cx| {
                        view.update(cx, |this, cx| match event {
                            native::Event::Toolbar(visible) => {
                                this.toolbar_visible = visible;
                                cx.notify();
                            }
                            native::Event::Toggle => {
                                if window.is_window_active() {
                                    this.hide(cx);
                                } else {
                                    cx.activate(true);
                                    window.activate_window();
                                }
                            }
                            native::Event::SwipeLeft
                                if !this.searching && !this.account.visible =>
                            {
                                this.step(1, window, cx)
                            }
                            native::Event::SwipeRight
                                if !this.searching && !this.account.visible =>
                            {
                                this.step(-1, window, cx)
                            }
                            _ => {}
                        })
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        window.on_window_should_close(cx, |_, cx| {
            cx.hide();
            false
        });
        let mut this = Self {
            account: account::Account::new(cx),
            editors: Default::default(),
            deletions: Vec::new(),
            store,
            current: None,
            editor,
            query,
            editor_events: None,
            searching: false,
            slide_direction: None,
            selected: 0,
            pending_delete: None,
            scroll: ScrollHandle::new(),
            error: None,
            shortcut_available,
            toolbar_visible: false,
        };
        this.attach_editor(window, cx);
        this.start_sync(window, cx);
        this
    }

    fn attach_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_events = Some(cx.subscribe_in(
            &self.editor,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.save(cx);
                    cx.notify();
                }
            },
        ));
        window.focus(&self.editor.focus_handle(cx), cx);
    }

    fn save(&mut self, cx: &mut Context<Self>) -> bool {
        let body = self.editor.read(cx).value();
        let title = body
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .trim_start_matches('#')
            .trim();
        let title: String = title.chars().take(100).collect();
        let result = match self.current {
            Some(id) if self.store.get(id).is_some_and(|n| n.body == body.as_ref()) => {
                self.error = None;
                return true;
            }
            Some(id) => self.store.update(id, &title, &body),
            None if body.trim().is_empty() => {
                self.error = None;
                return true;
            }
            None => self
                .store
                .create_with_body(&title, &body)
                .map(|id| self.current = Some(id)),
        };
        if result.is_ok() {
            self.schedule_sync(cx);
        }
        self.error = result
            .err()
            .map(|e| format!("Couldn’t save this note · {e}"));
        self.error.is_none()
    }

    fn can_create_note(&self, cx: &App) -> bool {
        !self.editor.read(cx).value().trim().is_empty()
    }

    fn open(&mut self, id: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        if id.is_none() && self.current.is_some() && !self.can_create_note(cx) {
            return;
        }
        if !self.save(cx) {
            cx.notify();
            return;
        }
        self.slide_direction = None;
        if self.current != id {
            if let Some(previous) = self.current {
                self.editors.insert(previous, self.editor.clone());
            }
            self.editor = id
                .and_then(|id| self.editors.remove(&id))
                .unwrap_or_else(|| {
                    let body = id.and_then(|id| self.store.get(id)).map_or("", |n| &n.body);
                    paper::editor_state(body, window, cx)
                });
            self.current = id;
            self.attach_editor(window, cx);
        }
        self.searching = false;
        window.focus(&self.editor.focus_handle(cx), cx);
        cx.notify();
    }

    fn ids(&self) -> Vec<u64> {
        let mut ids: Vec<_> = self.store.search("").into_iter().map(|n| n.id).collect();
        ids.sort_unstable();
        ids
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.ids();
        let pos = ids
            .iter()
            .position(|id| Some(*id) == self.current)
            .unwrap_or(ids.len());
        let next = pos.saturating_add_signed(delta).min(ids.len());
        if pos != next {
            let id = ids.get(next).copied();
            self.open(id, window, cx);
            if self.current == id {
                self.slide_direction = Some(delta);
            }
        }
    }

    fn search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.save(cx) {
            cx.notify();
            return;
        }
        self.query.update(cx, |s, cx| s.set_value("", window, cx));
        self.pending_delete = None;
        self.searching = true;
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        window.focus(&self.query.focus_handle(cx), cx);
        cx.notify();
    }

    fn hide(&mut self, cx: &mut Context<Self>) {
        if self.save(cx) {
            cx.hide();
        } else {
            cx.notify();
        }
    }

    fn key(&mut self, e: &KeyDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if self.account.visible {
            if k.key == "escape" {
                self.close_account(w, cx);
            } else if k.key == "enter" && self.store.active_account().is_none() {
                self.login(cx);
            } else {
                return;
            }
            cx.stop_propagation();
            w.prevent_default();
            return;
        }
        match (k.modifiers.platform, k.key.as_str()) {
            (true, "d")
                if !self.searching
                    && !k.modifiers.shift
                    && !k.modifiers.alt
                    && !k.modifiers.control =>
            {
                if !e.is_held
                    && let Some(id) = self.current
                {
                    self.remove_note(id, w, cx);
                }
            }
            (true, "z")
                if !k.modifiers.shift
                    && !k.modifiers.alt
                    && !k.modifiers.control
                    && self.can_restore(cx) =>
            {
                self.restore_note(w, cx);
            }
            (true, ",") => self.open_account(w, cx),
            (true, "f" | "p") => self.search(w, cx),
            (true, "n") => self.open(None, w, cx),
            (true, "w") | (false, "escape") if !self.searching => self.hide(cx),
            (true, "q") => {
                if self.save(cx) {
                    cx.quit();
                } else {
                    cx.notify();
                }
            }
            (false, "escape") => self.open(self.current, w, cx),
            (true, "[") if !self.searching => self.step(-1, w, cx),
            (true, "]") if !self.searching => self.step(1, w, cx),
            (false, "up" | "down" | "enter") if self.searching => {
                let notes = self.store.search(&self.query.read(cx).value());
                if k.key == "enter" {
                    if let Some(note) = notes.get(self.selected) {
                        self.open(Some(note.id), w, cx);
                    }
                } else {
                    let delta = if k.key == "up" { -1 } else { 1 };
                    self.selected = self
                        .selected
                        .saturating_add_signed(delta)
                        .min(notes.len().saturating_sub(1));
                    self.scroll.scroll_to_item(self.selected);
                    cx.notify();
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        w.prevent_default();
    }

    fn delete(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_delete != Some(id) {
            self.pending_delete = Some(id);
        } else {
            self.pending_delete = None;
            self.remove_note(id, window, cx);
        }
        cx.notify();
    }

    fn undo_context(&self, cx: &App) -> (EntityId, SharedString) {
        if self.searching {
            (self.query.entity_id(), self.query.read(cx).value())
        } else {
            (self.editor.entity_id(), self.editor.read(cx).value())
        }
    }

    fn can_restore(&self, cx: &App) -> bool {
        self.deletions.last().is_some_and(|entry| {
            let (target, text) = self.undo_context(cx);
            entry.undo_target == target && entry.target_text == text
        })
    }

    fn remove_note(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if !self.save(cx) {
            cx.notify();
            return;
        }
        match self.store.delete(id) {
            Ok(note) => {
                if self.current == Some(id) {
                    self.editors.insert(id, self.editor.clone());
                    self.current = None;
                    self.editor = paper::editor_state("", window, cx);
                    self.attach_editor(window, cx);
                    if self.searching {
                        window.focus(&self.query.focus_handle(cx), cx);
                    } else {
                        let ids = self.ids();
                        let next = ids
                            .iter()
                            .copied()
                            .find(|other| *other > id)
                            .or_else(|| ids.last().copied());
                        self.open(next, window, cx);
                    }
                }
                let (undo_target, target_text) = self.undo_context(cx);
                self.deletions.push(DeletedNote {
                    note,
                    undo_target,
                    target_text,
                });
                self.selected = 0;
                self.pending_delete = None;
                self.error = None;
                self.schedule_sync(cx);
            }
            Err(error) => self.error = Some(format!("Couldn’t delete this note · {error}")),
        }
        cx.notify();
    }

    fn restore_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.deletions.last() else {
            return;
        };
        let id = entry.note.id;
        match self.store.restore(&entry.note) {
            Ok(()) => {
                self.deletions.pop();
                self.error = None;
                self.schedule_sync(cx);
                if !self.searching {
                    self.open(Some(id), window, cx);
                }
            }
            Err(error) => self.error = Some(format!("Couldn’t restore this note · {error}")),
        }
        cx.notify();
    }

    fn browser(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.query.read(cx).value();
        let notes = self.store.search(&query);
        let mut list = div()
            .id("results")
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0();
        if notes.is_empty() {
            list = list.child(
                div()
                    .p_5()
                    .text_color(rgb(0x756e68))
                    .child(if query.is_empty() {
                        "Your saved notes will appear here."
                    } else {
                        "No matching notes."
                    }),
            );
        }
        for (index, n) in notes.into_iter().enumerate() {
            let id = n.id;
            let date = DateTime::from_timestamp_millis(n.updated_at as i64)
                .map(|d| {
                    d.with_timezone(&Local)
                        .format("%b %-d, %Y · %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            let preview = n
                .body
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("  |  ");
            list = list.child(
                div()
                    .id(("note", index))
                    .px_4()
                    .py_3()
                    .cursor_pointer()
                    .when(index == self.selected, |s| s.bg(rgb(0xf4f4f2)))
                    .hover(|s| s.bg(rgb(0xf5f5f3)))
                    .on_click(cx.listener(move |this, _, w, cx| this.open(Some(id), w, cx)))
                    .child(
                        div()
                            .font_family("Menlo")
                            .text_size(px(13.))
                            .line_height(px(19.))
                            .line_clamp(3)
                            .child(if preview.is_empty() {
                                "Untitled note".into()
                            } else {
                                preview
                            }),
                    )
                    .child(
                        div()
                            .mt_1()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_xs()
                            .text_color(rgb(0x7d7872))
                            .child(format!("Edited {date}"))
                            .child(tool(
                                if self.pending_delete == Some(id) {
                                    "Confirm delete"
                                } else {
                                    "Delete"
                                },
                                cx.listener(move |this, _, w, cx| {
                                    cx.stop_propagation();
                                    this.delete(id, w, cx);
                                }),
                            )),
                    ),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .mx_4()
                    .mb_4()
                    .px_2()
                    .py_1()
                    .rounded_lg()
                    .bg(rgb(0xf2f2f1))
                    .flex()
                    .items_center()
                    .child(Input::new(&self.query).appearance(false).flex_1())
                    .child(tool(
                        "esc",
                        cx.listener(|this, _, w, cx| this.open(this.current, w, cx)),
                    )),
            )
            .child(list)
            .into_any_element()
    }
}

fn tool(
    label: &'static str,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(label)
        .cursor_pointer()
        .rounded_md()
        .px_2()
        .py_1()
        .text_size(px(12.))
        .text_color(rgb(0x57534e))
        .hover(|s| s.bg(rgb(0xecece9)))
        .on_click(click)
        .child(label)
}

impl Render for Notebook {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut view = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(white())
            .text_color(rgb(0x443e37))
            .relative()
            .capture_action(cx.listener(|this, _: &kit::base::input::Undo, window, cx| {
                if !this.account.visible && this.can_restore(cx) {
                    this.restore_note(window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_key_down(cx.listener(Self::key));
        view = if self.account.visible {
            view.child(self.account_panel(cx))
        } else if self.searching {
            view.child(div().h(px(44.)).flex_shrink_0())
                .child(self.browser(cx))
        } else {
            view.child(paper::paper(&self.editor, self.slide_direction, cx))
        };
        if self.toolbar_visible && !self.account.visible {
            use kit::component::{Icon, IconName};
            let action = |id: &'static str, enabled| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .h(px(24.))
                    .rounded_md()
                    .when(enabled, |s| {
                        s.cursor_pointer().hover(|s| s.bg(rgb(0xeeeeec)))
                    })
                    .when(!enabled, |s| s.opacity(0.35))
            };
            let can_create_note = self.can_create_note(cx);
            view = view.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(54.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(white().opacity(0.98), 0.25),
                        linear_color_stop(white().opacity(0.), 1.),
                    ))
                    .flex()
                    .items_start()
                    .pt(px(12.))
                    .pr(px(12.))
                    .child(
                        div()
                            .h(px(24.))
                            .flex_1()
                            .on_mouse_down(MouseButton::Left, |_, w, _| w.start_window_move()),
                    )
                    .child(tool(
                        "Account",
                        cx.listener(|s, _, w, cx| s.open_account(w, cx)),
                    ))
                    .child(
                        action("search-notes", true)
                            .child(Icon::new(IconName::Search).size(px(14.)))
                            .child(div().text_xs().child("⌘F"))
                            .on_click(cx.listener(|s, _, w, cx| s.search(w, cx))),
                    )
                    .child(
                        action("new-note", can_create_note)
                            .child(Icon::new(IconName::Plus).size(px(16.)))
                            .when(can_create_note, |s| {
                                s.on_click(cx.listener(|s, _, w, cx| s.open(None, w, cx)))
                            }),
                    )
                    .with_animation(
                        "toolbar-reveal",
                        Animation::new(std::time::Duration::from_millis(110)),
                        |bar, t| bar.opacity(t),
                    ),
            );
        }
        if let Some(msg) = &self.error {
            view = view.child(
                div()
                    .px_3()
                    .text_xs()
                    .text_color(rgb(0x9b3224))
                    .child(msg.clone()),
            );
        }
        if !self.shortcut_available {
            view =
                view.child(
                    div().px_3().pb_2().text_xs().child(
                        "Option–N is unavailable. Check other apps for a shortcut conflict.",
                    ),
                );
        }
        view
    }
}

fn main() {
    let store = NoteStore::open_default().unwrap_or_else(|e| {
        eprintln!("Could not open HandyNotes: {e}");
        std::process::exit(1)
    });
    kit::application()
        .with_assets(kit::assets::Assets)
        .run(move |cx| {
            kit::init(cx);
            Theme::change(ThemeMode::Light, None, cx);
            let bounds = Bounds::centered(None, size(px(480.), px(590.)), cx);
            kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(320.), px(220.))),
                    app_owns_titlebar_drag: true,
                    titlebar: Some(TitlebarOptions {
                        title: Some("HandyNotes".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(16.), px(16.))),
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Notebook::new(store, window, cx)),
            )
            .expect("open HandyNotes window");
            cx.activate(true);
        });
}

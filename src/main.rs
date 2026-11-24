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

mod native;
mod paper;
mod store;

struct Notebook {
    store: NoteStore,
    current: Option<u64>,
    editor: Entity<EditorState>,
    query: Entity<InputState>,
    editor_events: Option<Subscription>,
    searching: bool,
    selected: usize,
    pending_delete: Option<u64>,
    scroll: ScrollHandle,
    error: Option<String>,
    shortcut_available: bool,
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
        let shortcut_available = native::install(tx);
        cx.spawn_in(window, async move |view, cx| {
            while let Ok(event) = rx.recv().await {
                if cx
                    .update(|window, cx| {
                        view.update(cx, |this, cx| match event {
                            native::Event::Toggle => {
                                cx.activate(true);
                                window.activate_window();
                                this.open(None, window, cx);
                            }
                            native::Event::SwipeLeft if !this.searching => this.step(1, window, cx),
                            native::Event::SwipeRight if !this.searching => {
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
            store,
            current: None,
            editor,
            query,
            editor_events: None,
            searching: false,
            selected: 0,
            pending_delete: None,
            scroll: ScrollHandle::new(),
            error: None,
            shortcut_available,
        };
        this.attach_editor(window, cx);
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

    fn save(&mut self, cx: &App) -> bool {
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
            Some(id) if self.store.get(id).is_some_and(|n| n.body == body.as_ref()) => Ok(()),
            Some(id) => self.store.update(id, &title, &body),
            None if body.is_empty() => Ok(()),
            None => self
                .store
                .create_with_body(&title, &body)
                .map(|id| self.current = Some(id)),
        };
        self.error = result
            .err()
            .map(|e| format!("Couldn’t save this note · {e}"));
        self.error.is_none()
    }

    fn open(&mut self, id: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.save(cx) {
            cx.notify();
            return;
        }
        if self.current != id {
            let body = id.and_then(|id| self.store.get(id)).map_or("", |n| &n.body);
            self.editor = paper::editor_state(body, window, cx);
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
            self.open(ids.get(next).copied(), window, cx);
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

    fn key(&mut self, e: &KeyDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        match (k.modifiers.platform, k.key.as_str()) {
            (true, "f" | "p") => self.search(w, cx),
            (true, "n") => self.open(None, w, cx),
            (true, "w") | (false, "escape") if !self.searching => {
                if self.save(cx) {
                    cx.hide();
                } else {
                    cx.notify();
                }
            }
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
            match self.store.delete(id) {
                Ok(()) => {
                    if self.current == Some(id) {
                        self.current = None;
                        self.editor = paper::editor_state("", window, cx);
                        self.attach_editor(window, cx);
                        window.focus(&self.query.focus_handle(cx), cx);
                    }
                    self.selected = 0;
                    self.error = None;
                }
                Err(error) => self.error = Some(format!("Couldn’t delete this note · {error}")),
            }
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
            .capture_key_down(cx.listener(Self::key))
            .child(
                div()
                    .h(px(27.))
                    .flex_shrink_0()
                    .on_mouse_down(MouseButton::Left, |_, w, _| w.start_window_move()),
            );
        view = if self.searching {
            view.child(self.browser(cx))
        } else {
            let ids = self.ids();
            let pos = ids.iter().position(|id| Some(*id) == self.current);
            let count = pos
                .map(|i| format!("{} / {}", i + 1, ids.len()))
                .unwrap_or_else(|| "New note".into());
            view.child(paper::paper(&self.editor)).child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(tool("‹", cx.listener(|s, _, w, cx| s.step(-1, w, cx))))
                            .child(div().text_xs().text_color(rgb(0x807c75)).child(count))
                            .child(tool("›", cx.listener(|s, _, w, cx| s.step(1, w, cx)))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(tool(
                                "Search ⌘F",
                                cx.listener(|s, _, w, cx| s.search(w, cx)),
                            ))
                            .child(tool(
                                "+ New",
                                cx.listener(|s, _, w, cx| s.open(None, w, cx)),
                            )),
                    ),
            )
        };
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
                    titlebar: Some(TitlebarOptions {
                        title: Some("HandyNotes".into()),
                        appears_transparent: true,
                        ..Default::default()
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

use super::*;
use crate::sync::{Event, Request, SessionChange};
use std::time::Duration;

const SYNC_DELAY: Duration = Duration::from_millis(350);
const POLL_INTERVAL: Duration = Duration::from_secs(4);

pub(super) struct Account {
    pub visible: bool,
    focus: FocusHandle,
    logout_requested: bool,
    requests: Option<async_channel::Sender<Request>>,
    busy: bool,
    status: String,
    version: Option<f64>,
    session_change: Option<SessionChange>,
    scheduled_sync: Option<Task<()>>,
}

impl Account {
    pub fn new(cx: &mut Context<Notebook>) -> Self {
        Self {
            visible: false,
            focus: cx.focus_handle(),
            logout_requested: false,
            requests: None,
            busy: false,
            status: String::new(),
            version: None,
            session_change: None,
            scheduled_sync: None,
        }
    }
}

impl Notebook {
    pub(super) fn start_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = std::env::var("CONVEX_URL")
            .ok()
            .or_else(|| option_env!("CONVEX_URL").map(str::to_owned));
        let Some(url) = url.filter(|url| !url.trim().is_empty()) else {
            self.account.status =
                "Set CONVEX_URL when building or launching HandyNotes to enable sync.".into();
            return;
        };
        let (requests, events) = sync::connect(url);
        self.account.requests = Some(requests);
        self.account.busy = true;
        cx.spawn_in(window, async move |view, cx| {
            while let Ok(event) = events.recv().await {
                if cx
                    .update(|w, cx| view.update(cx, |this, cx| this.cloud_event(event, w, cx)))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |view, cx| {
            loop {
                executor.timer(POLL_INTERVAL).await;
                if cx
                    .update(|w, cx| {
                        view.update(cx, |this, cx| {
                            if this.account.session_change.is_some() && !this.account.busy {
                                this.apply_session_change(w, cx);
                            }
                            this.sync_notes();
                        })
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn schedule_sync(&mut self, cx: &mut Context<Self>) {
        if self.store.active_account().is_none() {
            return;
        }
        let delay = cx.background_executor().timer(SYNC_DELAY);
        self.account.scheduled_sync = Some(cx.spawn(async move |view, cx| {
            delay.await;
            let _ = view.update(cx, |this, _| this.sync_notes());
        }));
    }

    fn request(&mut self, request: Request) {
        if self.account.busy {
            return;
        }
        if let Some(tx) = &self.account.requests {
            match tx.try_send(request) {
                Ok(()) => self.account.busy = true,
                Err(_) => self.account.status = "Cloud connection unavailable. Restart HandyNotes after checking the deployment URL.".into(),
            }
        }
    }

    fn logout(&mut self, cx: &mut Context<Self>) {
        self.account.logout_requested = true;
        self.account.status = "Signing out…".into();
        if !self.account.busy {
            self.account.logout_requested = false;
            self.request(Request::Logout);
        }
        cx.notify();
    }

    fn sync_notes(&mut self) {
        if !self.account.busy
            && !self.account.logout_requested
            && self.account.session_change.is_none()
            && let Some(account) = self.store.active_account()
        {
            self.request(Request::Sync {
                account: account.to_owned(),
                pending: self.store.pending(),
                version: self.account.version,
            });
        }
    }

    fn sync_status(&self) -> String {
        if self.store.pending().is_empty() {
            "Up to date".into()
        } else {
            "Changes waiting to sync".into()
        }
    }

    fn cloud_event(&mut self, event: Event, w: &mut Window, cx: &mut Context<Self>) {
        self.account.busy = matches!(&event, Event::OpenBrowser(_));
        match event {
            Event::OpenBrowser(url) => {
                self.account.status = "Finish signing in with GitHub in your browser.".into();
                cx.open_url(&url);
            }
            Event::Current => self.account.status = self.sync_status(),
            Event::PullFailed {
                account,
                sent,
                error,
            } => {
                self.account.status = self
                    .store
                    .acknowledge(&account, &sent)
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or(error);
            }
            Event::Error(message) => self.account.status = message,
            Event::Session(change) => {
                self.account.session_change = Some(change);
                if !self.account.logout_requested {
                    self.apply_session_change(w, cx);
                    self.sync_notes();
                }
            }
            Event::Snapshot {
                version,
                account,
                sent,
                notes,
                upload_error,
            } => {
                if self.store.active_account() == Some(&account) && self.save(cx) {
                    if let Err(error) = self.store.apply_sync(&account, &sent, notes) {
                        self.account.status = error.to_string();
                    } else {
                        self.account.version = Some(version);
                        self.editors.retain(|id, editor| {
                            self.store
                                .get(*id)
                                .is_some_and(|n| n.body == editor.state.read(cx).value().as_ref())
                        });
                        if let Some(id) = self.current {
                            let note = self.store.get(id);
                            if note.is_none_or(|n| {
                                n.body != self.editor.state.read(cx).value().as_ref()
                            }) {
                                self.current = note.map(|n| n.id);
                                self.editor = paper::editor_state(
                                    note.map_or("", |n| n.body.as_str()),
                                    w,
                                    cx,
                                );
                                self.attach_editor(w, cx);
                                if self.account.visible {
                                    w.focus(&self.account.focus, cx);
                                } else if self.searching {
                                    w.focus(&self.query.focus_handle(cx), cx);
                                }
                            }
                        }
                        self.account.status = upload_error.unwrap_or_else(|| self.sync_status());
                    }
                }
            }
        }
        if self.account.logout_requested {
            self.logout(cx);
        }
        cx.notify();
    }

    fn apply_session_change(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(change) = self.account.session_change.take() else {
            return;
        };
        if !self.save(cx) {
            self.account.status = format!(
                "{} The account switch will retry.",
                self.error
                    .as_deref()
                    .unwrap_or("Couldn’t save the open note.")
            );
            self.account.session_change = Some(change);
            cx.notify();
            return;
        }

        let switching = change.account.as_deref() != self.store.active_account();
        let result = match &change.account {
            Some(owner) => self.store.sign_in(owner, change.adoption.as_deref()),
            None => self.store.sign_out(),
        };
        if let Err(error) = result {
            self.account.status = format!("Couldn’t switch notebooks. Retrying… · {error}");
            self.account.session_change = Some(change);
            cx.notify();
            return;
        }

        self.account.version = None;
        if switching {
            self.editors.clear();
            self.deletions.clear();
            self.searching = false;
            self.pending_delete = None;
            self.selected = 0;
            self.current = None;
            self.query.update(cx, |s, cx| s.set_value("", w, cx));
            self.editor = paper::editor_state("", w, cx);
            self.attach_editor(w, cx);
            if self.account.visible {
                w.focus(&self.account.focus, cx);
            }
        }
        self.account.status = if change.account.is_some() {
            "Syncing…".into()
        } else {
            "Saved on this Mac".into()
        };
        cx.notify();
    }

    pub(super) fn open_account(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.save(cx) {
            self.account.visible = true;
            w.focus(&self.account.focus, cx);
        }
        cx.notify();
    }

    pub(super) fn close_account(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.account.visible = false;
        if self.searching {
            w.focus(&self.query.focus_handle(cx), cx);
        } else {
            w.focus(&self.editor.state.focus_handle(cx), cx);
        }
        cx.notify();
    }

    pub(super) fn login(&mut self, cx: &mut Context<Self>) {
        if self.account.busy
            || self.account.requests.is_none()
            || self.account.session_change.is_some()
        {
            return;
        }
        self.account.status = "Connecting…".into();
        self.request(Request::Login);
        cx.notify();
    }

    pub(super) fn account_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut panel = div()
            .track_focus(&self.account.focus)
            .mt(px(44.))
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child("Cloud sync")
                    .child(tool(
                        "Done",
                        cx.listener(|s, _, w, cx| s.close_account(w, cx)),
                    )),
            );
        if self.account.requests.is_some() {
            if self.store.active_account().is_some()
                || self
                    .account
                    .session_change
                    .as_ref()
                    .is_some_and(|change| change.account.is_some())
            {
                panel = panel.child(tool("Sign out", cx.listener(|s, _, _, cx| s.logout(cx))));
            } else {
                panel =
                    panel
                        .when(self.store.has_local_notes(), |p| {
                            p.child(div().text_xs().child(
                                "Signing in moves this Mac’s local notes into your account.",
                            ))
                        })
                        .child(tool(
                            "Sign in with GitHub",
                            cx.listener(|s, _, _, cx| s.login(cx)),
                        ));
            }
        }
        panel
            .child(div().text_xs().child(self.account.status.clone()))
            .into_any_element()
    }
}

//! macOS input events for the application, installed and delivered on the UI thread.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Event {
    Toggle,
    Toolbar(bool),
    SwipeLeft,
    SwipeRight,
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Event;
    use async_channel::Sender;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{cell::RefCell, ffi::c_void, ptr::NonNull};

    thread_local! {
        static EVENTS: RefCell<Option<Sender<Event>>> = const { RefCell::new(None) };
    }

    unsafe extern "C" {
        fn handynotes_configure_window(view: *mut c_void);
        fn handynotes_window_is_on_active_space(view: *mut c_void) -> bool;
        fn handynotes_input_install(deliver: extern "C" fn(i32)) -> i32;
    }

    extern "C" fn deliver(raw_event: i32) {
        let event = match raw_event {
            1 => Event::Toggle,
            2 => Event::SwipeLeft,
            3 => Event::SwipeRight,
            4 => Event::Toolbar(true),
            5 => Event::Toolbar(false),
            _ => return,
        };
        let _ = EVENTS.try_with(|events| {
            if let Some(sender) = events.borrow().as_ref() {
                let _ = sender.try_send(event);
            }
        });
    }

    fn appkit_view(window: &gpui_kit::Window) -> Option<NonNull<c_void>> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::AppKit(handle) => Some(handle.ns_view),
            _ => None,
        }
    }

    pub(crate) fn is_on_active_space(window: &gpui_kit::Window) -> bool {
        appkit_view(window)
            .is_none_or(|view| unsafe { handynotes_window_is_on_active_space(view.as_ptr()) })
    }

    /// Installs the swipe monitor and reports whether the global shortcut registered.
    pub(crate) fn install(window: &gpui_kit::Window, sender: Sender<Event>) -> bool {
        EVENTS.set(Some(sender));
        // AppKit and Carbon invoke this callback on the installing UI thread.
        let installed = unsafe { handynotes_input_install(deliver) == 0 };
        if let Some(view) = appkit_view(window) {
            unsafe { handynotes_configure_window(view.as_ptr()) };
        }
        installed
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::{install, is_on_active_space};

#[cfg(not(target_os = "macos"))]
pub(crate) fn is_on_active_space(_window: &gpui_kit::Window) -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn install(_window: &gpui_kit::Window, _sender: async_channel::Sender<Event>) -> bool {
    false
}

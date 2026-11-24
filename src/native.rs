//! macOS input events for the application, installed and delivered on the UI thread.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Event {
    Toggle,
    SwipeLeft,
    SwipeRight,
}

#[cfg(target_os = "macos")]
mod macos {
    use super::Event;
    use async_channel::Sender;
    use std::cell::RefCell;

    thread_local! {
        static EVENTS: RefCell<Option<Sender<Event>>> = const { RefCell::new(None) };
    }

    unsafe extern "C" {
        fn handynotes_input_install(deliver: extern "C" fn(i32)) -> i32;
    }

    extern "C" fn deliver(raw_event: i32) {
        let event = match raw_event {
            1 => Event::Toggle,
            2 => Event::SwipeLeft,
            3 => Event::SwipeRight,
            _ => return,
        };
        let _ = EVENTS.try_with(|events| {
            if let Some(sender) = events.borrow().as_ref() {
                let _ = sender.try_send(event);
            }
        });
    }

    /// Installs the swipe monitor and reports whether the global shortcut registered.
    pub(crate) fn install(sender: Sender<Event>) -> bool {
        EVENTS.set(Some(sender));
        // AppKit and Carbon invoke this callback on the installing UI thread.
        unsafe { handynotes_input_install(deliver) == 0 }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::install;

#[cfg(not(target_os = "macos"))]
pub(crate) fn install(_sender: async_channel::Sender<Event>) -> bool {
    false
}

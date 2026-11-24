# HandyNotes

A small macOS scratchpad built with Rust, GPUI and GPUI Kit. While HandyNotes is running, **Option–N** hides the active window or brings it back from another application. Your current note or search stays open. Typing saves it locally. Closing the window or pressing Escape hides it and keeps the shortcut active.

- Swipe horizontally with two fingers to browse one note at a time. Swipe right for an older note and left for a newer one. Move the pointer into the top row to reveal the macOS window controls, Search (⌘F), and New (+). Command–[ / Command–] also browse notes.
- Press **Command–F** to search all notes by their contents. Use the arrow keys and Return, or click a result. Escape returns to the editor. Click Delete twice to confirm removal.
- Press **Command–D** to delete the open note and **Command–Z** to restore it. Each note keeps its own editing history while the app is running, including when you switch notes.
- Press **Command–N** for a new note or **Command–Q** to quit. Drag the top edge to move the window, or drag a corner to resize it.

Notes live in `~/Library/Application Support/HandyNotes/notes.json`. Blank drafts are saved only after typing. A shortcut conflict appears at the bottom of the window if another application owns Option–N.

## Build on macOS

With Rust and Xcode installed, run:

```sh
scripts/build-app
open target/HandyNotes.app
```

You can move the resulting bundle into Applications. For development, run `cargo run --locked` or build the bundle with `scripts/build-app debug`. Run the storage tests with `cargo test --locked`.

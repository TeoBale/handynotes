# HandyNotes

A small macOS scratchpad built with Rust, GPUI and GPUI Kit. While HandyNotes is running, **Option–N** brings a blank note to the front from any application. Typing saves it locally. Closing the window or pressing Escape hides it and keeps the shortcut active.

- Swipe horizontally with two fingers to browse one note at a time. Swipe right for an older note and left for a newer one. The footer arrows and Command–[ / Command–] do the same.
- Press **Command–F** to search all notes by their contents. Use the arrow keys and Return, or click a result. Escape returns to the editor. Click Delete twice to confirm removal.
- Press **Command–N** for a new note or **Command–Q** to quit. Drag the top edge to move the window, or drag a corner to resize it.

Notes live in `~/Library/Application Support/HandyNotes/notes.json`. Blank drafts are saved only after typing. A shortcut conflict appears at the bottom of the window if another application owns Option–N.

## Build on macOS

With Rust and Xcode installed, run:

```sh
scripts/build-app
open target/HandyNotes.app
```

You can move the resulting bundle into Applications. For development, run `cargo run --locked` or build the bundle with `scripts/build-app debug`. Run the storage tests with `cargo test --locked`.

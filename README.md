# HandyNotes

A small macOS scratchpad built with Rust, GPUI and GPUI Kit. While HandyNotes is running, **Option–N** hides the active window or brings it back from another application. Your current note or search stays open. Typing saves it locally. Closing the window or pressing Escape hides it and keeps the shortcut active.

- Swipe horizontally with two fingers to browse one note at a time. Swipe right for an older note and left for a newer one. Move the pointer into the top row to reveal the macOS window controls, Search (⌘F), and New (+). Command–[ / Command–] also browse notes.
- Press **Command–F** to search all notes by their contents. Use the arrow keys and Return, or click a result. Escape returns to the editor. Click Delete twice to confirm removal.
- Press **Command–D** to delete the open note and **Command–Z** to restore it. Each note keeps its own editing history while the app is running, including when you switch notes.
- Press **Command–N** for a new note once the current page contains text, or **Command–Q** to quit. Drag the top edge to move the window, or drag a corner to resize it.

Notes live in `~/Library/Application Support/HandyNotes/notes.json`. Blank drafts are saved only after typing. A shortcut conflict appears at the bottom of the window if another application owns Option–N.

## Build on macOS

With Rust and Xcode installed, run:

```sh
scripts/build-app
open target/HandyNotes.app
```

You can move the resulting bundle into Applications. For development, run `cargo run --locked` or build the bundle with `scripts/build-app debug`. Run the storage tests with `cargo test --locked`.

## Sync across Macs with Convex

Each Mac connects to the same Convex deployment. Open **Account** in the top toolbar (or press **Command–comma**), create an account on the first Mac, and sign in with the same email and password on the others. Signing in imports that Mac’s local notes into the account. Signing out returns to the local notebook and keeps the account’s cached notes and pending edits on disk for the next sign-in.

Notes save locally as you type. Once you sign in, HandyNotes automatically uploads edits and deletions in the background. It checks for updates every four seconds and retries after a connection failure. Closing the window keeps sync running. Quitting the app pauses it. If two devices edit the same note, the last upload accepted by Convex wins, including an edit made offline. The **Account** panel shows connection errors and sync status. Credentials for the session are stored in the macOS login Keychain.

### Connect a deployment

1. From the repository, run `npm ci` and `npx convex dev`. Sign in to Convex and select or create a hosted project. Keep the development command running during setup.
2. In a second terminal in the same directory, run `npx @convex-dev/auth@0.0.98` and follow the prompts to configure the deployment’s signing keys. This app uses the email/password provider with no browser redirect. For a production deployment, configure auth with `--prod` and publish the functions with `npm run deploy`.
3. Build the app from the repository. The build reads the public `CONVEX_URL` from your shell, then `.env.local`, then `.env`. Finder launches use that embedded URL. Use the same build on each Mac.

```sh
scripts/build-app
open target/HandyNotes.app
```

For development, `CONVEX_URL=https://YOUR-DEPLOYMENT.convex.cloud cargo run --locked` overrides the built-in URL. Without a deployment URL, HandyNotes runs locally. This version supports password sign-up and sign-in. Email verification and password recovery are not configured.

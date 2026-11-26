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

## Optional sync and web access

HandyNotes works offline without an account. To sync, open **Account** on your Mac (⌘,) and select **Sign in with GitHub**. Finish in your browser and return to HandyNotes. Signing in moves that Mac’s local notebook into the selected account. Signing out returns to the local notebook and keeps the account’s cached notes and pending edits on disk for the next sign-in. Session tokens are stored in the macOS login Keychain.

Notes save locally as you type. When signed in, HandyNotes uploads edits and deletions in the background, checks for updates every four seconds and retries after a connection failure. Closing the window keeps sync running. Quitting pauses it. If two devices edit the same note, the last upload accepted by Convex wins. The **Account** panel shows sync status.

Sign in to the web app with the same GitHub account to read your synced notes. Search by title or content, then select a result to open it beside the list.

### Configure GitHub sign-in

1. Run `npm ci` at the repository root. For a new project, select a development deployment with `npx convex dev`.
2. New deployments need Convex Auth signing keys. Use `npx @convex-dev/auth@0.0.98` to configure them. Keep existing signing keys when upgrading.
3. Register a GitHub OAuth app with your web origin as the homepage (`http://localhost:5173` for local development). Set the authorization callback URL to the deployment’s **HTTP Actions URL** followed by `/api/auth/callback/github`. Both clients use this callback. Convex returns desktop users to a temporary loopback address on their Mac. [GitHub OAuth app registration](https://github.com/settings/applications/new)
4. In the Convex deployment’s environment settings, set `AUTH_GITHUB_ID` and `AUTH_GITHUB_SECRET` to the OAuth app credentials and `SITE_URL` to the web origin. Only the backend needs the client secret. Publish the development functions with `npx convex dev --once`. [Convex GitHub setup guide](https://labs.convex.dev/auth/config/oauth/github)
5. Build the desktop app with `scripts/build-app`. It reads the public `CONVEX_URL` from your shell, then `.env.local`, then `.env`. Finder launches use the embedded URL. During development, you can override it with `CONVEX_URL=https://YOUR-DEPLOYMENT.convex.cloud cargo run --locked`.

### Run the web app

From the repository root, after `npm ci`:

```sh
cd web
bun install --frozen-lockfile
bun run dev --port 5173 --strictPort
```

The web app uses the root project’s public `CONVEX_URL`. To override it, set `VITE_CONVEX_URL` in `web/.env.local` or the build environment. Use the web origin configured as `SITE_URL` when opening the app. Run `bun run test` and `bun run build` from `web/`. When serving the production build, configure the host to serve `index.html` for client-side routes such as `/login`.

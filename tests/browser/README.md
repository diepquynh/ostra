# Browser security suite

Proves the browser-facing defenses hold in a real Chromium: render paths for agent and repo text,
the terminal, the cross-origin boundary, the sign-in token, uploads, and browser storage.

```bash
cd tests/browser && npm test
```

`npm test` builds the web UI and `target-browser/debug/ostra`, starts a scratch server under
`/tmp/pw-browser` (its own config, data dir, master key, and `HOME`), and stops it by pid at the
end. Nothing leaves the machine: the model API is a local fake (`lib/fake-model.ts`) that plays a
whole YOLO session, the harness is a stub script, and the MCP server is a stub.

- `npm run test:fast` skips the build, for reruns after a spec change.
- `PW_KEEP_SERVER=1` leaves the scratch server running after the run (`/tmp/pw-browser/server.pid`).
- `PW_CHROME=/path/to/chrome` overrides the Chromium binary (default: the cached `chromium-1217`).

Every test string would set `window.__pwned` or request `<fake>/canary/<id>` if it ran or loaded.
The `guard` fixture (`lib/guard.ts`) fails a test on either, on any CSP violation on the app
origin (printed with the blocked URL, whose id names the render path), and on any unexpected
dialog, popup, or navigation away from the app.

| Spec | Covers |
| --- | --- |
| `render` | Agent messages, tool output (Bash, MCP), artifacts as documents and Markdown, the completion report, session titles, repo file names and contents, branches and commits, MCP metadata, memory, skills, decisions, search, the quick answer |
| `terminal` | OSC 52, OSC 8, title sequences, and device queries through xterm.js; the PTY input must match a run with no browser attached |
| `cross-origin` | Attacker pages on another localhost port and another site: forms, fetch, WebSocket, tags, frames, and a navigation to every GET route in `api.rs`; DNS rebinding; page content reaching `/api` |
| `signin` | The token leaves the URL, no request carries it, and what the profile keeps is spent |
| `uploads` | SVG and HTML uploads never render inline, through chips, the artifact view, or raw links |
| `storage` | localStorage, sessionStorage, IndexedDB, and Cache Storage hold no transcripts, tokens, or keys |

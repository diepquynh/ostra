# notes

A notes app: a JSON API on `node:http` and a page in `public/` that uses it. Node 24, no runtime dependencies.

- `src/notes.js`: the rules for notes. `src/store.js`: an in-memory store.
- `server/app.js`: the request handler (the API and the page's files). `server/main.js` starts it:
  `PORT=3000 node server/main.js`.
- `public/index.html` and `public/app.js`: the page. It lists notes and adds new ones through the API.

Tests: `test/unit/` (node:test) and `test/integration/` (the real app on a local port). `.ostra/project.toml` has
the command for each. There are no browser tests.

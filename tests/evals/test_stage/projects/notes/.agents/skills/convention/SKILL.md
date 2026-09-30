---
name: convention
description: How code in notes is written. Load for any change.
---

# notes conventions

- ES modules, Node 24, no runtime dependencies.
- Rules live in `src/notes.js` and throw `RangeError` for bad input and `NotFoundError` for a missing note.
  `server/app.js` maps them to 400 and 404; nothing else catches them.
- The page in `public/` only calls the API; it holds no rules of its own.

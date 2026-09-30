// The request handler: the notes API under /api, and the page's files from public/.

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { createNote, listNotes, NotFoundError } from "../src/notes.js";

const PUBLIC = {
  "/": ["index.html", "text/html"],
  "/app.js": ["app.js", "text/javascript"],
};

function send(res, status, body) {
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
}

async function readJson(req) {
  let raw = "";
  for await (const chunk of req) {
    raw += chunk;
  }
  return raw ? JSON.parse(raw) : {};
}

export function createApp(store) {
  return async (req, res) => {
    const url = new URL(req.url, "http://localhost");
    try {
      if (req.method === "GET" && url.pathname === "/api/notes") {
        return send(res, 200, listNotes(store));
      }
      if (req.method === "POST" && url.pathname === "/api/notes") {
        const { text } = await readJson(req);
        return send(res, 201, createNote(store, text));
      }
      const file = req.method === "GET" ? PUBLIC[url.pathname] : undefined;
      if (file) {
        const body = await readFile(fileURLToPath(new URL(`../public/${file[0]}`, import.meta.url)));
        res.writeHead(200, { "Content-Type": file[1] });
        return res.end(body);
      }
      return send(res, 404, { error: "not found" });
    } catch (error) {
      if (error instanceof RangeError || error instanceof SyntaxError) {
        return send(res, 400, { error: error.message });
      }
      if (error instanceof NotFoundError) {
        return send(res, 404, { error: error.message });
      }
      return send(res, 500, { error: "internal error" });
    }
  };
}

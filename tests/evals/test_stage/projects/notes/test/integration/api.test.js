import assert from "node:assert/strict";
import test from "node:test";
import { post, withServer } from "./support.js";

test("POST /api/notes creates a note that GET /api/notes lists", () =>
  withServer(async (url) => {
    const created = await post(`${url}/api/notes`, { text: "milk" });
    assert.equal(created.status, 201);
    const notes = await (await fetch(`${url}/api/notes`)).json();
    assert.deepEqual(
      notes.map((note) => note.text),
      ["milk"],
    );
  }));

test("POST /api/notes with empty text is 400", () =>
  withServer(async (url) => {
    const response = await post(`${url}/api/notes`, { text: " " });
    assert.equal(response.status, 400);
  }));

test("GET / serves the page", () =>
  withServer(async (url) => {
    const response = await fetch(`${url}/`);
    assert.equal(response.status, 200);
    assert.match(await response.text(), /<ul id="notes">/);
  }));

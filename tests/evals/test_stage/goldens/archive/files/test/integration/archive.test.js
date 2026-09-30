import assert from "node:assert/strict";
import test from "node:test";
import { post, withServer } from "./support.js";

test("POST /api/notes/:id/archive archives the note and GET /api/notes leaves it out", () =>
  withServer(async (url) => {
    const note = await (await post(`${url}/api/notes`, { text: "milk" })).json();
    const response = await post(`${url}/api/notes/${note.id}/archive`);
    assert.equal(response.status, 200);
    assert.equal((await response.json()).archived, true);
    const listed = await (await fetch(`${url}/api/notes`)).json();
    assert.deepEqual(listed, []);
  }));

test("GET /api/notes?archived=true lists the archived notes", () =>
  withServer(async (url) => {
    const note = await (await post(`${url}/api/notes`, { text: "milk" })).json();
    await post(`${url}/api/notes/${note.id}/archive`);
    const listed = await (await fetch(`${url}/api/notes?archived=true`)).json();
    assert.deepEqual(
      listed.map((n) => n.text),
      ["milk"],
    );
  }));

test("archiving an unknown note is 404", () =>
  withServer(async (url) => {
    const response = await post(`${url}/api/notes/42/archive`);
    assert.equal(response.status, 404);
  }));

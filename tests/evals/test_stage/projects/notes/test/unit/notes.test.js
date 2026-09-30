import assert from "node:assert/strict";
import test from "node:test";
import { createNote, listNotes } from "../../src/notes.js";
import { MemoryStore } from "../../src/store.js";

function clock() {
  let t = 0;
  return () => ++t;
}

test("createNote trims the text and stores the note unarchived", () => {
  const store = new MemoryStore(clock());
  const note = createNote(store, "  milk  ");
  assert.equal(note.text, "milk");
  assert.equal(note.archived, false);
  assert.deepEqual(store.get(note.id), note);
});

test("createNote rejects empty text", () => {
  assert.throws(() => createNote(new MemoryStore(clock()), "   "), RangeError);
});

test("listNotes returns the newest note first", () => {
  const store = new MemoryStore(clock());
  createNote(store, "first");
  createNote(store, "second");
  assert.deepEqual(
    listNotes(store).map((note) => note.text),
    ["second", "first"],
  );
});

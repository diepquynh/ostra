import assert from "node:assert/strict";
import test from "node:test";
import { archiveNote, createNote, listNotes, NotFoundError } from "../../src/notes.js";
import { MemoryStore } from "../../src/store.js";

function clock() {
  let t = 0;
  return () => ++t;
}

test("archiveNote marks the note archived and records when", () => {
  const store = new MemoryStore(clock());
  const note = createNote(store, "milk");
  const archived = archiveNote(store, note.id);
  assert.equal(archived.archived, true);
  assert.equal(archived.archivedAt, 2);
  assert.equal(store.get(note.id).archived, true);
});

test("archiving an archived note keeps the first archive time", () => {
  const store = new MemoryStore(clock());
  const note = createNote(store, "milk");
  const first = archiveNote(store, note.id);
  const second = archiveNote(store, note.id);
  assert.equal(second.archivedAt, first.archivedAt);
});

test("archiveNote of an unknown id throws NotFoundError", () => {
  assert.throws(() => archiveNote(new MemoryStore(clock()), 42), NotFoundError);
});

test("listNotes leaves archived notes out by default", () => {
  const store = new MemoryStore(clock());
  const kept = createNote(store, "keep");
  const gone = createNote(store, "gone");
  archiveNote(store, gone.id);
  assert.deepEqual(
    listNotes(store).map((note) => note.id),
    [kept.id],
  );
});

test("listNotes with archived true lists only the archived notes", () => {
  const store = new MemoryStore(clock());
  createNote(store, "keep");
  const gone = createNote(store, "gone");
  archiveNote(store, gone.id);
  assert.deepEqual(
    listNotes(store, { archived: true }).map((note) => note.id),
    [gone.id],
  );
});

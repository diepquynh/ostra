// The rules for notes. The store keeps them; this module decides what is allowed.

export class NotFoundError extends Error {}

export function createNote(store, text) {
  const trimmed = String(text ?? "").trim();
  if (trimmed === "") {
    throw new RangeError("a note needs text");
  }
  if (trimmed.length > 280) {
    throw new RangeError("a note is at most 280 characters");
  }
  return store.insert({ text: trimmed, archived: false, createdAt: store.now() });
}

export function listNotes(store) {
  return store.all().sort((a, b) => b.createdAt - a.createdAt);
}

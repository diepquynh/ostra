// An in-memory store of notes. `now` is injectable, so tests control the order notes are listed in.

export class MemoryStore {
  constructor(now = () => Date.now()) {
    this.notes = new Map();
    this.nextId = 1;
    this.now = now;
  }

  insert(fields) {
    const note = { id: this.nextId++, ...fields };
    this.notes.set(note.id, note);
    return { ...note };
  }

  get(id) {
    const note = this.notes.get(Number(id));
    return note ? { ...note } : undefined;
  }

  update(id, fields) {
    const note = this.notes.get(Number(id));
    if (!note) {
      return undefined;
    }
    Object.assign(note, fields);
    return { ...note };
  }

  all() {
    return [...this.notes.values()].map((note) => ({ ...note }));
  }
}

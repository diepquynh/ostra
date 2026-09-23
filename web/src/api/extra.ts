// Types the generated bindings do not cover.

/** A decoded binary PTY frame from `/ws`. */
export type PtyFrame = { execution: string; data: Uint8Array };

/**
 * One changed file of a review loop, for the ledger's diff view. Served by
 * `GET /api/sessions/:id/diff?project=&phase=`, which the server may not implement yet; the
 * Artifacts screen falls back to markdown on 404.
 */
export type DiffFile = { path: string; original: string; modified: string };

/** Connection state of the socket manager. */
export type SocketState = "connecting" | "open" | "closed";

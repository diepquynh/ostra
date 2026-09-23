// Types the generated bindings do not cover.

/** A decoded binary PTY frame from `/ws`. */
export type PtyFrame = { execution: string; data: Uint8Array };

/** Connection state of the socket manager. */
export type SocketState = "connecting" | "open" | "closed";

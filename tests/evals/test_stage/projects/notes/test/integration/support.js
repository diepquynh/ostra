// Starts the real app on a free local port for one test, with a fresh store.

import { createServer } from "node:http";
import { createApp } from "../../server/app.js";
import { MemoryStore } from "../../src/store.js";

export async function withServer(run) {
  let t = 0;
  const server = createServer(createApp(new MemoryStore(() => ++t)));
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  try {
    await run(`http://127.0.0.1:${port}`);
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}

export async function post(url, body) {
  return fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
}

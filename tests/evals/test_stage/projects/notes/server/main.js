import { createServer } from "node:http";
import { MemoryStore } from "../src/store.js";
import { createApp } from "./app.js";

const port = Number(process.env.PORT ?? 3000);
createServer(createApp(new MemoryStore())).listen(port, "127.0.0.1", () => {
  console.log(`notes on http://127.0.0.1:${port}`);
});

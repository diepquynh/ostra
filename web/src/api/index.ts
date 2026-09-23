import { httpApi, type Api } from "./client";
import { mockApi } from "./mock/mockApi";

export const isMock = import.meta.env.VITE_MOCK === "1";

/** The API the app uses: the real server, or fixtures under `VITE_MOCK=1`. */
export const api: Api = isMock ? mockApi : httpApi;

export { HttpError, onUnauthorized } from "./client";
export { socket } from "./socket";

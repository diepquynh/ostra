import { type Api, httpApi } from "./client";

export const isMock = import.meta.env.VITE_MOCK === "1";

/**
 * The API the app uses: the real server, or fixtures under `VITE_MOCK=1`. The fixtures load with a dynamic import,
 * because a static one keeps them in the production bundle even though the branch is dead there.
 */
export const api: Api = isMock ? (await import("./mock/mockApi")).mockApi : httpApi;

export { HttpError, onUnauthorized } from "./client";
export { socket } from "./socket";

/** Where the browser downloads a session file (an upload or any other artifact). */
export const downloadUrl = (path: string) => `/api/artifacts/download?path=${encodeURIComponent(path)}`;

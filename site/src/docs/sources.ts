import claude from "../../../CLAUDE.md?raw";
import handover from "../../../HANDOVER.md?raw";
import readme from "../../../README.md?raw";
import browserTests from "../../../tests/browser/README.md?raw";
import { TEST_DOC_FILE } from "./pages";

const docsDir = import.meta.glob<string>("../../../docs/**/*.md", { query: "?raw", import: "default", eager: true });
const docsImages = import.meta.glob<string>("../../../docs/images/**/*.png", {
  query: "?url",
  import: "default",
  eager: true,
});
const fromRoot = (path: string) => path.replace(/^(\.\.\/)+/, "");

/** Repository Markdown the docs can show, keyed by its path from the repository root. Every file in docs/ is here. */
export const SOURCES: Record<string, string> = {
  "README.md": readme,
  "HANDOVER.md": handover,
  "CLAUDE.md": claude,
  "tests/browser/README.md": browserTests,
  ...Object.fromEntries(Object.entries(docsDir).map(([path, text]) => [fromRoot(path), text])),
  ...(import.meta.env.VITE_TEST_DOC ? { [TEST_DOC_FILE]: import.meta.env.VITE_TEST_DOC as string } : {}),
};

/** Images in docs/images, bundled into the site, keyed by their path from the repository root. */
export const IMAGES: Record<string, string> = Object.fromEntries(
  Object.entries(docsImages).map(([path, url]) => [fromRoot(path), url]),
);

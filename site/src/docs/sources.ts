import claude from "../../../CLAUDE.md?raw";
import handover from "../../../HANDOVER.md?raw";
import readme from "../../../README.md?raw";
import browserTests from "../../../tests/browser/README.md?raw";
import { TEST_DOC_FILE } from "./pages";

const docsDir = import.meta.glob<string>("../../../docs/**/*.md", { query: "?raw", import: "default", eager: true });

/** Repository Markdown the docs can show, keyed by its path from the repository root. Every file in docs/ is here. */
export const SOURCES: Record<string, string> = {
  "README.md": readme,
  "HANDOVER.md": handover,
  "CLAUDE.md": claude,
  "tests/browser/README.md": browserTests,
  ...Object.fromEntries(Object.entries(docsDir).map(([path, text]) => [path.replace(/^(\.\.\/)+/, ""), text])),
  ...(import.meta.env.VITE_TEST_DOC ? { [TEST_DOC_FILE]: import.meta.env.VITE_TEST_DOC as string } : {}),
};

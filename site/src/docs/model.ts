import { Docs as DocsBase, type NavGroup } from "@ostra/design/docs";
import { REPO_FILE } from "../shared/links";
import { NAV } from "./pages";
import { SOURCES } from "./sources";

export * from "@ostra/design/docs";

/** The site's docs: the repository Markdown in `NAV`, with links to other repository files going to GitHub. */
export class Docs extends DocsBase {
  constructor(nav: NavGroup[] = NAV, sources: Record<string, string> = SOURCES) {
    super(nav, sources, (file) => REPO_FILE + file);
  }
}

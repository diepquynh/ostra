// The docs renderer shared by the site's docs and the console's documentation books. Import "@ostra/design/docs.css"
// with it.
export { Diagram, type Theme } from "./Diagram";
export { Markdown } from "./Markdown";
export {
  blockText,
  Docs,
  type FileUrl,
  type Hit,
  type Link,
  MAX_HITS,
  type NavGroup,
  type Page,
  type PageDef,
  parseMarkdown,
  pickSection,
  resolvePath,
  slug,
  type Target,
  type TocEntry,
} from "./model";

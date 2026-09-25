import type { ContextFile } from "../../api/types";
import { Icon } from "../../design";
import { useNav, useShell } from "../../lib/nav";
import { baseName, fileKey, isFolder, splitTags, tagOf } from "./tags";
import "./context.css";

/**
 * A tagged file or folder as a compact chip: its name, the full tag on hover. A click opens the file, or the Files tab
 * on the folder's project.
 */
export function FileChip({ file, onRemove }: { file: ContextFile; onRemove?: () => void }) {
  const nav = useNav();
  const shell = useShell();
  const folder = isFolder(file);
  return (
    <span className="ctx-chip" title={tagOf(file)}>
      <button
        type="button"
        className="ctx-chip__open"
        onClick={() =>
          folder ? shell.browseFiles(file.project) : nav.open(`file:${file.project}:${file.path}`, { preview: true })
        }
      >
        <Icon name={folder ? "folder" : "file"} size={11} />
        {baseName(file.path)}
      </button>
      {onRemove && (
        <button type="button" className="ctx-chip__remove" aria-label={`Remove ${tagOf(file)}`} onClick={onRemove}>
          <Icon name="x" size={10} />
        </button>
      )}
    </span>
  );
}

/** Text with each `@project/path` tag of a listed file drawn as a chip. */
export function TaggedText({ text, files }: { text: string; files: ContextFile[] }) {
  const known = new Set(files.map(fileKey));
  const projects = [...new Set(files.map((f) => f.project))];
  return (
    <>
      {splitTags(text, projects, known).map((s, i) =>
        "file" in s ? <FileChip key={`${i}-${s.raw}`} file={s.file} /> : <span key={i}>{s.text}</span>,
      )}
    </>
  );
}

/** Files attached without a tag in the text, as a row of chips. */
export function UntaggedFiles({ text, files }: { text: string; files: ContextFile[] }) {
  const rest = files.filter((f) => !text.includes(tagOf(f)));
  if (rest.length === 0) return null;
  return (
    <span className="ctx-files">
      {rest.map((f) => (
        <FileChip key={fileKey(f)} file={f} />
      ))}
    </span>
  );
}

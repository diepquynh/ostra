import { useMemo, useState } from "react";
import type { TreeSession } from "../api/nav";
import type { ProjectView } from "../api/types";
import { CommandPalette, type PaletteItem } from "../design";
import { useFileIndex, useSearch } from "../lib/live";
import { localItems, mergePalette, paletteCommands } from "../lib/palette";

type PaletteProps = {
  ws: string;
  open: boolean;
  onClose: () => void;
  sessions: TreeSession[];
  projects: ProjectView[];
  /** The Files tab's project, searched locally when the server has no search endpoint. */
  filesProject: string | null;
  onSelect: (id: string) => void;
};

const passThrough = (items: PaletteItem[]) => items;

/** ⌘K: server search (debounced), local rows for an empty query, and the design's commands. */
export function Palette({ ws, open, onClose, sessions, projects, filesProject, onSelect }: PaletteProps) {
  const [query, setQuery] = useState("");
  const search = useSearch(ws, open ? query : "");
  const index = useFileIndex(ws, open && search.unavailable ? filesProject : null);
  const commands = useMemo(() => paletteCommands(), []);
  const local = useMemo(
    () => localItems(sessions, projects, search.unavailable && filesProject ? { key: filesProject, paths: index.paths } : null),
    [sessions, projects, search.unavailable, filesProject, index.paths],
  );
  const hits = search.unavailable || search.loading || !query.trim() ? null : search.items;
  const items = useMemo(() => mergePalette(query, hits, local, commands), [query, hits, local, commands]);
  return (
    <CommandPalette
      open={open}
      items={items}
      filterItems={passThrough}
      onQueryChange={setQuery}
      onClose={() => {
        setQuery("");
        onClose();
      }}
      onSelect={(it) => onSelect(it.id)}
    />
  );
}

import { useState } from "react";
import { api } from "../api";
import { useAsync } from "../lib/hooks";
import { ErrorBox, Loading } from "./Status";

/** Browse the server's filesystem and pick a directory. */
export function FolderPicker({ value, onChange }: { value: string; onChange: (path: string) => void }) {
  const [browse, setBrowse] = useState<string | undefined>(value || undefined);
  const listing = useAsync(() => api.listDir(browse), [browse]);

  return (
    <div className="stack">
      <div className="row">
        <input
          value={value}
          placeholder="/absolute/path/to/folder"
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              setBrowse(value);
            }
          }}
        />
        <button type="button" onClick={() => setBrowse(value || undefined)}>
          Browse
        </button>
      </div>
      {listing.error && <ErrorBox error={listing.error} onRetry={listing.reload} />}
      {listing.loading && !listing.data && <Loading />}
      {listing.data && (
        <div className="picker">
          <div className="picker-row muted small">
            <span className="mono ellipsis">{listing.data.path}</span>
            <span className="spacer" />
            <button type="button" className="small primary" onClick={() => onChange(listing.data!.path)}>
              Use this folder
            </button>
          </div>
          {listing.data.parent && (
            <div className="picker-row" onClick={() => setBrowse(listing.data!.parent!)}>
              <span>..</span>
            </div>
          )}
          {listing.data.entries
            .filter((e) => e.is_dir)
            .map((e) => {
              const path = `${listing.data!.path.replace(/\/$/, "")}/${e.name}`;
              return (
                <div key={e.name} className="picker-row" onClick={() => setBrowse(path)} onDoubleClick={() => onChange(path)}>
                  <span>{e.name}</span>
                  {e.is_git && <span className="chip">git</span>}
                  {e.is_ostra_project && <span className="chip ok">Ostra project</span>}
                </div>
              );
            })}
        </div>
      )}
    </div>
  );
}

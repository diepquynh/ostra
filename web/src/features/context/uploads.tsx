import { Icon, Spinner } from "@ostra/design";
import { useRef, useState } from "react";
import { api, downloadUrl } from "../../api";
import type { UploadedFile } from "../../api/types";
import { formatBytes } from "./tags";
import "./context.css";

export type PendingUpload = {
  key: string;
  name: string;
  size: number;
  /** The staged upload's id once the server has it. */
  id: string | null;
  error: string | null;
};

/** Files picked or dropped for a request, staged on the server as soon as they are chosen. */
export function useUploads(ws: string) {
  const [items, setItems] = useState<PendingUpload[]>([]);
  const seq = useRef(0);
  const patch = (key: string, p: Partial<PendingUpload>) =>
    setItems((xs) => xs.map((x) => (x.key === key ? { ...x, ...p } : x)));
  const add = (files: File[]) => {
    for (const file of files) {
      const key = `u${seq.current++}`;
      setItems((xs) => [...xs, { key, name: file.name, size: file.size, id: null, error: null }]);
      api.uploadFile(ws, file).then(
        (ref) => patch(key, { id: ref.id, name: ref.name }),
        (e: Error) => patch(key, { error: e.message }),
      );
    }
  };
  return {
    items,
    add,
    remove: (key: string) => setItems((xs) => xs.filter((x) => x.key !== key)),
    clear: () => setItems([]),
    /** Ids of the uploads ready to send. */
    ids: items.filter((x) => x.id && !x.error).map((x) => x.id as string),
    busy: items.some((x) => !x.id && !x.error),
  };
}

/** A file waiting to be sent with the request: its name, a spinner while it uploads, a remove button. */
export function PendingUploadChip({ upload, onRemove }: { upload: PendingUpload; onRemove: () => void }) {
  const title = upload.error ?? `${upload.name} · ${formatBytes(upload.size)}`;
  return (
    <span className={`ctx-chip ctx-chip--upload${upload.error ? " ctx-chip--error" : ""}`} title={title}>
      <span className="ctx-chip__open">
        {upload.id || upload.error ? <Icon name="paperclip" size={11} /> : <Spinner size={9} />}
        {upload.name}
      </span>
      <button type="button" className="ctx-chip__remove" aria-label={`Remove ${upload.name}`} onClick={onRemove}>
        <Icon name="x" size={10} />
      </button>
    </span>
  );
}

/** A kept upload as a chip that downloads it. */
export function UploadChip({ upload }: { upload: UploadedFile }) {
  return (
    <span className="ctx-chip ctx-chip--upload" title={`${upload.name} · ${formatBytes(upload.size)}`}>
      <a className="ctx-chip__open" href={downloadUrl(upload.path)} download={upload.name}>
        <Icon name="paperclip" size={11} />
        {upload.name}
      </a>
    </span>
  );
}

/** A button that opens the file picker and hands the chosen files over. */
export function UploadButton({ onFiles, disabled }: { onFiles: (files: File[]) => void; disabled?: boolean }) {
  const input = useRef<HTMLInputElement>(null);
  return (
    <>
      <button
        type="button"
        className="ctx-input__attach"
        disabled={disabled}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => input.current?.click()}
      >
        <Icon name="paperclip" size={12} /> Upload a file
      </button>
      <input
        ref={input}
        type="file"
        multiple
        hidden
        aria-label="Upload files"
        onChange={(e) => {
          onFiles(Array.from(e.target.files ?? []));
          e.target.value = "";
        }}
      />
    </>
  );
}

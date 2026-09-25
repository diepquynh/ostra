import { lazy, Suspense, useMemo, useState } from "react";
import { useLocation } from "react-router";
import { api } from "../api";
import { Banner, Breadcrumbs, Button, Chip, IconButton, Spinner } from "../design";
import { useAsync } from "../lib/hooks";
import { useNav, useShell } from "../lib/nav";
import { locationId } from "../lib/resource";
import { lineFromHash } from "./project/code/tokens";

export type DependencyFileScreenProps = {
  ws: string;
  projectKey: string;
  /** The language server's URI of the file. */
  uri: string;
};

const FileEditor = lazy(() => import("./project/code/FileEditor"));

const noop = () => {};

const LOCKED = "This file belongs to a dependency, so Ostra shows it read-only.";

/**
 * Resource `dep:<key>:<uri>`: a file outside the project that a language server pointed at, such as a library
 * source under a package cache or a class inside a jar, read-only. Ctrl/Cmd+click asks the same server, so
 * navigation continues through the library and back into the project. A `#L<n>` hash marks a line.
 */
export function DependencyFileScreen({ ws, projectKey, uri }: DependencyFileScreenProps) {
  const nav = useNav();
  const shell = useShell();
  const file = useAsync(() => api.codeExternal(ws, projectKey, uri), [ws, projectKey, uri]);
  const hashLine = lineFromHash(useLocation().hash);
  const reveal = useMemo(() => (hashLine ? { line: hashLine, n: 0 } : null), [hashLine]);
  const [copied, setCopied] = useState(false);
  const f = file.data;
  const copy = () => {
    if (!f) return;
    void navigator.clipboard?.writeText(f.path).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%", minHeight: 0 }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          height: 36,
          padding: "0 8px 0 12px",
          borderBottom: "1px solid var(--border-subtle)",
          flex: "none",
        }}
      >
        <div style={{ flex: 1, minWidth: 0, overflow: "hidden" }} title={f?.path ?? uri}>
          <Breadcrumbs
            onNavigate={(_, i) => i === 0 && nav.open(`project:${projectKey}`, { beside: true })}
            items={[
              { label: projectKey, icon: "folder-git-2" },
              { label: "Dependencies", icon: "package" },
              { label: f?.path ?? "…" },
            ]}
          />
        </div>
        {f && <Chip>{f.provider}</Chip>}
        <Chip>Read-only</Chip>
        <IconButton
          size="sm"
          icon="message-square"
          label="Ask about this file"
          onClick={() => shell.openDock(`About ${f?.path ?? uri} (a dependency of ${projectKey}): `)}
        />
        <IconButton
          size="sm"
          icon={copied ? "check" : "copy"}
          label={copied ? "Copied" : "Copy path"}
          onClick={copy}
          disabled={!f}
        />
      </div>
      <div style={{ flex: 1, minHeight: 0 }}>
        {file.error ? (
          <div style={{ padding: 16 }}>
            <Banner
              tone="bad"
              actions={
                <Button size="sm" onClick={file.reload}>
                  Try again
                </Button>
              }
            >
              {file.error.message}
            </Banner>
          </div>
        ) : !f ? (
          <div style={{ padding: 20, display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
            <Spinner size={11} /> Reading the file…
          </div>
        ) : (
          <Suspense
            fallback={
              <div style={{ padding: 20, display: "flex", gap: 8, alignItems: "center", color: "var(--text-muted)" }}>
                <Spinner size={11} /> Loading the editor…
              </div>
            }
          >
            <FileEditor
              ws={ws}
              projectKey={projectKey}
              path={f.path}
              value={f.content}
              onChange={noop}
              theme={shell.theme}
              onSave={noop}
              dirty={false}
              onOpen={(to) => nav.open(locationId(projectKey, to), { anchor: `L${to.line}`, beside: true })}
              readOnly
              locked={LOCKED}
              onEditAt={noop}
              reveal={reveal}
              external={{ uri, name: f.name }}
            />
          </Suspense>
        )}
      </div>
    </div>
  );
}

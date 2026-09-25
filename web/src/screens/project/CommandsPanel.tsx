import { Fragment, useState } from "react";
import { api, HttpError } from "../../api";
import type { Commands } from "../../api/gen/Commands";
import { Banner, Button, Input, Panel } from "../../design";

export const COMMANDS: [keyof Commands, string, string][] = [
  ["build", "Build", "cargo build"],
  ["test", "Test", "cargo test"],
  ["test_one", "One test", "cargo test {name}"],
  ["typecheck", "Typecheck", "tsc --noEmit"],
  ["lint", "Lint", "cargo clippy -- -D warnings"],
  ["format", "Format", "cargo fmt"],
  ["run", "Run", "cargo run"],
];

/** Per-field messages from a 422, keyed by command (`commands.build` becomes `build`), and the rest. */
export function commandErrors(e: unknown): { fields: Partial<Record<keyof Commands, string>>; general: string | null } {
  const issues = e instanceof HttpError ? e.issues : [];
  const fields: Partial<Record<keyof Commands, string>> = {};
  const rest: string[] = [];
  for (const i of issues) {
    const k = i.path.replace(/^commands\./, "") as keyof Commands;
    if (COMMANDS.some(([c]) => c === k)) fields[k] = i.message;
    else rest.push(i.message);
  }
  const general = rest.length ? rest.join(" ") : issues.length ? null : e instanceof Error ? e.message : String(e);
  return { fields, general };
}

/**
 * The project's commands from `.ostra/project.toml`, editable in place. Agents and the engine read the
 * profile per execution, so a saved command applies to the next run. `onSaved` refetches the workspace.
 */
export function CommandsPanel({ ws, projectKey, commands, onSaved }: { ws: string; projectKey: string; commands: Commands; onSaved: () => void }) {
  const [draft, setDraft] = useState<Commands | null>(null);
  const [saving, setSaving] = useState(false);
  const [errors, setErrors] = useState<ReturnType<typeof commandErrors> | null>(null);
  const set = COMMANDS.filter(([k]) => commands[k]);

  const start = () => {
    setDraft({ ...commands });
    setErrors(null);
  };
  const save = () => {
    if (!draft) return;
    setSaving(true);
    setErrors(null);
    api.saveProjectCommands(ws, projectKey, draft).then(
      () => {
        setDraft(null);
        onSaved();
      },
      (e: unknown) => setErrors(commandErrors(e)),
    ).finally(() => setSaving(false));
  };

  const actions = draft ? null : (
    <Button size="sm" variant="ghost" icon="pencil" onClick={start}>
      Edit
    </Button>
  );

  return (
    <Panel title="Commands" icon="square-terminal" actions={actions}>
      {draft ? (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
          style={{ display: "flex", flexDirection: "column", gap: 10 }}
        >
          {errors?.general && <Banner tone="bad">{errors.general}</Banner>}
          {COMMANDS.map(([k, label, example]) => (
            <Input
              key={k}
              label={label}
              mono
              size="sm"
              placeholder={example}
              hint={k === "test_one" ? "{name} is replaced with the test to run." : undefined}
              error={errors?.fields[k] ?? null}
              value={draft[k] ?? ""}
              onChange={(e) => setDraft({ ...draft, [k]: e.target.value })}
            />
          ))}
          <div style={{ fontSize: "var(--text-sm)", color: "var(--text-muted)" }}>
            Saved to <code>.ostra/project.toml</code>. Leave a field empty to remove that command. Agents use the new commands from their next run.
          </div>
          <div style={{ display: "flex", gap: 8 }}>
            <Button variant="primary" size="sm" type="submit" disabled={saving}>
              {saving ? "Saving…" : "Save commands"}
            </Button>
            <Button variant="ghost" size="sm" disabled={saving} onClick={() => setDraft(null)}>
              Cancel
            </Button>
          </div>
        </form>
      ) : set.length ? (
        <div style={{ display: "grid", gridTemplateColumns: "80px 1fr", gap: "8px 10px", alignItems: "center", fontSize: "var(--text-sm)" }}>
          {set.map(([k, label]) => (
            <Fragment key={k}>
              <span style={{ color: "var(--text-muted)" }}>{label}</span>
              <code style={{ wordBreak: "break-all" }}>{commands[k]}</code>
            </Fragment>
          ))}
        </div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 8, alignItems: "flex-start", fontSize: "var(--text-sm)", color: "var(--text-muted)" }}>
          <span>
            No commands in <code>.ostra/project.toml</code>. Agents build, test, and format with these.
          </span>
          <Button size="sm" icon="plus" onClick={start}>
            Add commands
          </Button>
        </div>
      )}
    </Panel>
  );
}

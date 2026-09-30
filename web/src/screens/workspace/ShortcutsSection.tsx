import { Banner, Button, Kbd, Panel, Table } from "@ostra/design";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { manualInstallHint, useInstall } from "../../lib/install";
import {
  ACTIONS,
  type Action,
  conflicts,
  defaults,
  type Keys,
  keysHint,
  type Overrides,
  reservedBy,
  strokeCaps,
  strokeOf,
  typesText,
  useShortcuts,
} from "../../lib/shortcuts";

/** A first stroke becomes a one-key shortcut when no second stroke follows within this time. */
const SECOND_STROKE_MS = 1200;

const labelOf = (a: Action) => ACTIONS.find((x) => x.id === a)?.label ?? a;
const sameKeys = (a: Keys | undefined, b: Keys | undefined) =>
  !!a && !!b && a.length === b.length && a.every((s, i) => s === b[i]);

type Pending = { action: Action; keys: Keys; taken: Action[] };
type Row = { id: Action; label: string };

/** Settings tab `shortcuts`: the console's shortcuts for this workspace, kept in this browser only. */
export function ShortcutsSection({ ws }: { ws: string }) {
  const { bindings, overrides, set, reset } = useShortcuts(ws);
  const [recording, setRecording] = useState<Action | null>(null);
  const [strokes, setStrokes] = useState<Keys>([]);
  const [pending, setPending] = useState<Pending | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const base = defaults();

  const stopTimer = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
  };
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  const warningsFor = (keys: Keys): string | null => {
    const notes: string[] = [];
    const by = reservedBy(keys[0]);
    if (by === "browser")
      notes.push(
        `The browser may keep ${keysHint(keys)} for itself. It reaches Ostra while the keyboard is locked in fullscreen or Ostra runs as an installed app.`,
      );
    if (by === "system")
      notes.push(`The operating system may handle ${keysHint(keys)} before the browser, so Ostra may never see it.`);
    if (typesText(keys[0]))
      notes.push("It works only outside text fields and editors, because typing there would trigger it.");
    return notes.length ? notes.join(" ") : null;
  };

  const apply = (action: Action, keys: Keys, unbind: Action[] = []) => {
    const changes: Overrides = { [action]: sameKeys(keys, base[action]?.keys) ? undefined : keys };
    for (const other of unbind) changes[other] = base[other] ? null : undefined;
    set(changes);
    setNotice(warningsFor(keys));
  };

  const finish = (action: Action, keys: Keys) => {
    stopTimer();
    setRecording(null);
    setStrokes([]);
    if (keys.length === 0) return;
    const taken = conflicts(bindings, action, keys);
    if (taken.length) setPending({ action, keys, taken });
    else apply(action, keys);
  };

  const start = (action: Action) => {
    stopTimer();
    setPending(null);
    setNotice(null);
    setStrokes([]);
    setRecording(action);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (!recording) return;
    e.preventDefault();
    // The console's own shortcuts must not run while a new one is being pressed.
    e.stopPropagation();
    const s = strokeOf(e.nativeEvent);
    if (!s) return;
    if (s === "Escape" && strokes.length === 0) {
      setRecording(null);
      return;
    }
    const next = [...strokes, s];
    if (next.length === 2) return finish(recording, next);
    setStrokes(next);
    stopTimer();
    const action = recording;
    timer.current = setTimeout(() => finish(action, next), SECOND_STROKE_MS);
  };

  const columns = [
    { key: "label", label: "Action" },
    {
      key: "keys",
      label: "Shortcut",
      width: 280,
      render: (r: Row) => {
        if (recording === r.id)
          return (
            <span
              ref={(el) => el?.focus()}
              tabIndex={0}
              role="textbox"
              aria-label={`Press the new shortcut for ${r.label}`}
              className="os-input os-input--sm"
              style={{ display: "inline-flex", gap: 4, minWidth: 200, outline: "2px solid var(--accent)" }}
              onKeyDown={onKeyDown}
              onBlur={() => finish(r.id, strokes)}
            >
              {strokes.length ? (
                <>
                  <Strokes keys={strokes} />
                  <span className="wp-muted">then another key, or wait</span>
                </>
              ) : (
                <span className="wp-muted">Press keys, or Esc to cancel</span>
              )}
            </span>
          );
        const b = bindings[r.id];
        if (!b) return <span className="wp-muted">Not set</span>;
        const by = reservedBy(b.keys[0]);
        return (
          <span style={{ display: "inline-flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
            <Strokes keys={b.keys} />
            {b.reserved ? (
              <span className="wp-muted">while locked or installed</span>
            ) : by ? (
              <span className="wp-muted">
                {by === "browser" ? "the browser may keep it" : "the system may keep it"}
              </span>
            ) : null}
          </span>
        );
      },
    },
    {
      key: "actions",
      label: "",
      width: 230,
      render: (r: Row) => (
        <span className="wp-row" style={{ justifyContent: "flex-end", gap: 4 }}>
          <Button size="sm" onClick={() => start(r.id)} disabled={recording === r.id}>
            {bindings[r.id] ? "Change" : "Set"}
          </Button>
          {bindings[r.id] && (
            <Button size="sm" variant="ghost" onClick={() => set({ [r.id]: base[r.id] ? null : undefined })}>
              Remove
            </Button>
          )}
          {r.id in overrides && (
            <Button size="sm" variant="ghost" onClick={() => reset(r.id)}>
              Reset
            </Button>
          )}
        </span>
      ),
    },
  ];

  return (
    <>
      <ShortcutLimits />
      <Panel
        title="Keyboard shortcuts"
        subtitle="saved in this browser for this workspace; changes apply at once"
        actions={
          Object.keys(overrides).length > 0 ? (
            <Button size="sm" variant="ghost" icon="rotate-ccw" onClick={() => reset()}>
              Reset all
            </Button>
          ) : undefined
        }
      >
        <div className="wp-stack">
          <p className="wp-lead">
            Click Set or Change, then press the keys. A shortcut can be one combination, such as Ctrl+Alt+P, or two
            pressed one after the other, such as Ctrl+K then S. Other workspaces and other browsers keep their own
            shortcuts.
          </p>
          {pending && (
            <Banner
              tone="warn"
              actions={
                <>
                  <Button
                    size="sm"
                    variant="primary"
                    onClick={() => {
                      apply(pending.action, pending.keys, pending.taken);
                      setPending(null);
                    }}
                  >
                    Use it here
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setPending(null)}>
                    Cancel
                  </Button>
                </>
              }
            >
              {keysHint(pending.keys)} is used by {pending.taken.map(labelOf).join(" and ")}. Using it for{" "}
              {labelOf(pending.action)} removes it there.
            </Banner>
          )}
          {notice && <Banner tone="info">{notice}</Banner>}
          <Table<Row> columns={columns} rows={ACTIONS} dense />
        </div>
      </Panel>
    </>
  );
}

/** The keys of a shortcut, with "then" between the steps of a sequence. */
function Strokes({ keys }: { keys: Keys }) {
  return keys.map((s, i) => (
    <span key={`${i}:${s}`} style={{ display: "inline-flex", gap: 4, alignItems: "center" }}>
      {i > 0 && <span className="wp-muted">then</span>}
      <Kbd keys={strokeCaps(s)} />
    </span>
  ));
}

/** The note about combos Ostra cannot take, with the install button that frees most browser ones. */
function ShortcutLimits() {
  const { state, install } = useInstall();
  const [hint, setHint] = useState<string | null>(null);
  return (
    <Banner
      tone="info"
      title="Some combos stay with the browser or the system"
      actions={
        state === "installed" ? undefined : (
          <Button
            size="sm"
            icon="download"
            onClick={() => void install().then((shown) => setHint(shown ? null : manualInstallHint()))}
          >
            Install Ostra as an app
          </Button>
        )
      }
    >
      Ostra's shortcuts cannot override the browser's built-in shortcuts, such as the ones that open or close browser
      tabs, or the operating system's shortcuts, because those handle the key press before the page sees it.{" "}
      {state === "installed"
        ? "Ostra is running as an installed app, so the browser passes most of its own shortcuts to Ostra."
        : "To use the most shortcuts, install Ostra as a browser app. It then runs in its own window, where the browser passes most of its shortcuts to Ostra."}
      {hint && <div style={{ marginTop: 6 }}>{hint}</div>}
    </Banner>
  );
}

import { Banner, Button, Panel, Table } from "@ostra/design";
import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import type { SignInSession } from "../../api/types";

const when = (iso: string) => new Date(iso).toLocaleString();

/** A short browser name from a User-Agent, falling back to the raw string. */
function browserOf(ua: string | null): string {
  if (!ua) return "Unknown browser";
  const name = /(Edg|Firefox|Chrome|Safari)\/[\d.]+/.exec(ua)?.[1];
  const os = /(Windows|Mac OS X|Android|iPhone|Linux)/.exec(ua)?.[1];
  const label = name === "Edg" ? "Edge" : name;
  return label ? `${label}${os ? ` on ${os === "Mac OS X" ? "macOS" : os}` : ""}` : ua;
}

/**
 * The browsers signed in to this server, for every workspace. Revoking one signs it out at once: its next request
 * is refused and its open tabs disconnect.
 */
export function SignInSessions() {
  const [list, setList] = useState<SignInSession[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const load = useCallback(() => {
    api.signIns().then(setList, (e: Error) => setError(e.message));
  }, []);
  useEffect(load, [load]);
  const run = async (work: Promise<unknown>, done?: string) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await work;
      if (done) setNotice(done);
      load();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const others = (list ?? []).filter((s) => !s.current).length;

  return (
    <Panel
      title="Sign-in sessions"
      subtitle="every browser signed in to this server; the same list for every workspace"
      bodyFlush
      actions={
        <Button
          size="sm"
          variant="danger"
          disabled={busy || others === 0}
          onClick={() =>
            void run(
              api.revokeOtherSignIns().then((r) => r.revoked),
              "Every other browser is signed out.",
            )
          }
        >
          Sign out everywhere else
        </Button>
      }
    >
      {error && <Banner tone="bad">{error}</Banner>}
      {notice && <Banner tone="info">{notice}</Banner>}
      <Table<SignInSession>
        dense
        rows={list ?? []}
        empty={list ? "No browser is signed in." : "Loading…"}
        columns={[
          {
            key: "browser",
            label: "Browser",
            render: (s) => (
              <span title={s.user_agent ?? undefined}>
                {browserOf(s.user_agent)}
                {s.current && <strong> (this browser)</strong>}
              </span>
            ),
          },
          {
            key: "ip",
            label: "Address",
            width: 140,
            render: (s) => <span className="wp-mono">{s.ip ?? "unknown"}</span>,
          },
          { key: "created", label: "Signed in", width: 170, render: (s) => when(s.created) },
          { key: "last_seen", label: "Last seen", width: 170, render: (s) => when(s.last_seen) },
          {
            key: "actions",
            label: "",
            width: 110,
            render: (s) =>
              s.current ? null : (
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  title={`Sign out ${browserOf(s.user_agent)}`}
                  onClick={() => void run(api.revokeSignIn(s.id), "That browser is signed out.")}
                >
                  Revoke
                </Button>
              ),
          },
        ]}
      />
      <p style={{ margin: "10px 12px", color: "var(--text-secondary)", lineHeight: 1.55 }}>
        Locked out, or a browser you no longer have? On the machine that runs Ostra, run <code>ostra sessions</code> to
        list sign-ins and <code>ostra sessions revoke --all</code> to sign out every browser.
      </p>
    </Panel>
  );
}

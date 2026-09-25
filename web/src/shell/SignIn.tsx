import { type FormEvent, useEffect, useState } from "react";
import { exchangeToken, tokenFromInput } from "../auth";
import { Banner, Button, Input, Panel } from "../design";
import { applyTheme, lastTheme, resolveTheme } from "../lib/theme";
import "./shell.css";

/** Shown when the server answers 401: this browser has no session cookie. */
export function SignIn({ error }: { error: string | null }) {
  const [link, setLink] = useState("");
  const [message, setMessage] = useState<string | null>(error);
  const [busy, setBusy] = useState(false);
  useEffect(() => applyTheme(resolveTheme(lastTheme()), false), []);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const token = tokenFromInput(link);
    if (!token) {
      setMessage("Paste the whole link the ostra command printed, or only the token after #token=.");
      return;
    }
    setBusy(true);
    const err = await exchangeToken(token);
    setBusy(false);
    if (err) setMessage(err);
    else location.replace("/");
  };

  return (
    <div style={{ height: "100%", overflow: "auto", background: "var(--surface-editor)" }}>
      <div
        style={{
          maxWidth: 560,
          margin: "0 auto",
          padding: "80px 24px",
          display: "flex",
          flexDirection: "column",
          gap: 16,
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 9,
            font: "600 15px/1 var(--font-sans)",
            letterSpacing: "0.02em",
          }}
        >
          <img src="/favicon.svg" width={20} height={20} alt="" />
          Ostra
        </div>
        <Panel title="Sign in to Ostra" icon="shield-check">
          <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            <p style={{ margin: 0, lineHeight: 1.55, color: "var(--text-secondary)" }}>
              This browser has no session with the Ostra server. Open the URL that the <code>ostra</code> command
              printed in your terminal, or paste it below. It carries a one-time token that signs this browser in.
            </p>
            <div style={{ display: "flex", gap: 8 }}>
              <Input
                mono
                aria-label="Sign-in link"
                placeholder="http://…/#token=…"
                value={link}
                onChange={(e) => setLink(e.target.value)}
                style={{ flex: 1 }}
                autoFocus
              />
              <Button type="submit" variant="primary" disabled={busy || !link.trim()}>
                Sign in
              </Button>
            </div>
            <p style={{ margin: 0, fontSize: "var(--text-sm)", color: "var(--text-muted)", lineHeight: 1.5 }}>
              Each link works once and expires after 15 minutes. Run <code>ostra url</code> on the server for a new one;
              the server keeps running.
            </p>
            {message && (
              <Banner tone="bad" style={{ margin: 0 }}>
                {message}
              </Banner>
            )}
          </form>
        </Panel>
      </div>
    </div>
  );
}

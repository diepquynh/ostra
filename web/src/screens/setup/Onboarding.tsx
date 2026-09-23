import { Stepper } from "../../design";
import { useInitAfterCreate } from "./finish";
import { WizardBody, WizardNav } from "./steps";
import { useWizard } from "./useWizard";

export type OnboardingProps = {
  /**
   * The setup ended: with the new workspace's id, or null when the user skipped it. The caller marks
   * onboarding complete (`POST /api/onboarding/complete`) and navigates.
   */
  onFinish: (workspaceId: string | null) => void;
};

/**
 * Full-screen first-run setup, shown when `GET /api/onboarding` has no `onboarded_at` and no workspace
 * exists, and again from "Run the setup guide again": Welcome, Check this machine, Name and folder, Add
 * projects, Defaults, Review.
 */
export function Onboarding({ onFinish }: OnboardingProps) {
  const w = useWizard(false);
  const open = (id: string) => onFinish(id);
  const init = useInitAfterCreate((d) => open(d.id));
  const pct = ((w.i + (w.done ? 1 : 0)) / w.steps.length) * 100;
  return (
    <div style={{ position: "fixed", inset: 0, zIndex: 60, background: "var(--surface-chrome)", display: "flex", flexDirection: "column" }}>
      <div style={{ height: 3, background: "var(--border-subtle)", flex: "none" }}>
        <div style={{ height: "100%", width: `${pct}%`, background: "var(--accent)", transition: "width var(--dur-slow) var(--ease-out)" }} />
      </div>
      <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
        <aside
          style={{ width: 280, flex: "none", padding: "28px 20px", display: "flex", flexDirection: "column", gap: 28, borderRight: "1px solid var(--border-subtle)" }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: 9, font: "600 15px/1 var(--font-sans)", letterSpacing: "0.02em" }}>
            <img src="/favicon.svg" width={20} height={20} alt="" />
            Ostra
          </div>
          <Stepper steps={w.steps} current={w.creating || w.created ? w.steps.length - 1 : w.i} onSelect={w.creating || w.created ? undefined : w.go} />
          <span style={{ flex: 1 }} />
          <div style={{ fontSize: "var(--text-xs)", color: "var(--text-muted)", lineHeight: 1.5 }}>
            Signed in on {location.host}. Run <code>ostra url</code> for a new sign-in link.
          </div>
        </aside>
        <main style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", background: "var(--surface-editor)" }}>
          <div style={{ flex: 1, overflow: "auto" }}>
            <div style={{ maxWidth: 720, margin: "0 auto" }}>
              <WizardBody w={w} padding="48px 40px 40px" />
            </div>
          </div>
          <div
            style={{ display: "flex", alignItems: "center", gap: 8, padding: "12px 24px", borderTop: "1px solid var(--border-default)", background: "var(--surface-panel)" }}
          >
            <WizardNav w={w} onCancel={() => onFinish(null)} cancelLabel="Skip setup" onOpen={(d) => open(d.id)} onInit={init.init} initializing={init.busy} />
          </div>
        </main>
      </div>
    </div>
  );
}

import { Dialog, Stepper } from "@ostra/design";
import { useNavigate } from "react-router";
import type { WorkspaceDetail } from "../../api/types";
import { useInitAfterCreate } from "./finish";
import { WizardBody, WizardNav } from "./steps";
import { useWizard } from "./useWizard";

export type NewWorkspaceDialogProps = {
  onClose: () => void;
};

/**
 * The New workspace flow in a dialog, for users who already have a workspace: the setup steps minus
 * Welcome. On success it navigates to `/w/<new id>`, which remounts the shell.
 */
export function NewWorkspaceDialog({ onClose }: NewWorkspaceDialogProps) {
  const w = useWizard(true);
  const navigate = useNavigate();
  const open = (d: WorkspaceDetail) => {
    onClose();
    navigate(`/w/${encodeURIComponent(d.id)}`);
  };
  const init = useInitAfterCreate(onClose);
  const locked = w.creating || !!w.created;
  return (
    <Dialog
      title="New workspace"
      subtitle={`Step ${w.i + 1} of ${w.steps.length} · ${w.steps[w.i].label}`}
      onClose={w.creating && !w.done ? undefined : onClose}
      width={860}
      bodyStyle={{ padding: 0, display: "flex", minHeight: 480 }}
      footer={<WizardNav w={w} onCancel={onClose} onOpen={open} onInit={init.init} initializing={init.busy} />}
    >
      <div
        style={{
          width: 220,
          flex: "none",
          padding: 12,
          borderRight: "1px solid var(--border-subtle)",
          background: "var(--surface-panel)",
        }}
      >
        <Stepper steps={w.steps} current={locked ? w.steps.length - 1 : w.i} onSelect={locked ? undefined : w.go} />
      </div>
      <div style={{ flex: 1, minWidth: 0, overflow: "auto" }}>
        <WizardBody w={w} padding="22px 24px 28px" />
      </div>
    </Dialog>
  );
}

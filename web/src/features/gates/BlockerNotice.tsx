import { Banner } from "@ostra/design";
import type { SessionEvent } from "../../api/types";
import { ReviewFindings } from "./Findings";

type Block = Extract<SessionEvent, { type: "security_block" }>;

/** A BLOCKER security finding. It has no dismiss action: only a passing review clears it (Hard 21). */
export function BlockerNotice({ block }: { block: Block }) {
  return (
    <Banner
      tone="bad"
      icon="shield-alert"
      title={`Security block in ${block.project}, phase ${block.phase}${block.tests ? " tests" : ""}`}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 4 }}>
        <p style={{ margin: 0 }}>
          The reviewer found code whose effect looks dangerous. It may not be intentional, because a weaker generation
          pass or a copied insecure example can produce it. Ostra sends only these findings back with an instruction to
          remove the code, and reviews again until they are gone. Nobody can waive this, and the session cannot complete
          while it is open.
        </p>
        <ReviewFindings findings={block.findings} />
        <p style={{ margin: 0, fontSize: "var(--text-sm)" }}>
          Read the Guidance text to learn what to research. A secure reimplementation is a separate request you make
          once you understand the risk.
        </p>
      </div>
    </Banner>
  );
}

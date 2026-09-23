import type { SessionEvent } from "../../api/types";
import { ReviewFindings } from "./Findings";

type Block = Extract<SessionEvent, { type: "security_block" }>;

/** A BLOCKER security finding. It has no dismiss button: only a passing review clears it. */
export function BlockerNotice({ block }: { block: Block }) {
  return (
    <div className="card danger" role="alert">
      <h3>
        Security block in {block.project}, phase {block.phase}
        {block.tests ? " tests" : ""}
      </h3>
      <p>
        The reviewer found code whose effect looks dangerous. It may not be intentional: a weaker generation pass or a copied insecure
        example can produce it. Ostra sends only these findings back with an instruction to remove the code, and reviews again until
        they are gone. Nobody can waive this, and the session cannot complete while it is open.
      </p>
      <ReviewFindings findings={block.findings} />
      <p className="small">
        The Guidance text says what to research. A secure reimplementation is a separate request you make once you understand the risk.
      </p>
    </div>
  );
}

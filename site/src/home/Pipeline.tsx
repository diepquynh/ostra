import {
  Button,
  GateCard,
  LiveMark,
  MARK_ARCS,
  type MarkArc,
  type MarkState,
  StatusDot,
  type Tone,
} from "@ostra/design";
import { useEffect, useRef, useState } from "react";

const NAMES = ["research", "spec", "fact-check", "plan", "build", "review", "test", "docs"];

// Lane copy from web/src/content/stages.ts: each lane's `why`, and its main stage's produces and protects against.
// The site cannot import from web/, so a change there needs the same change here.
const LANES = [
  {
    title: "Research",
    why: "Code written before the problem is understood carries wrong assumptions that nobody challenges until they are hundreds of lines deep. A grounded research pass reads the code and the current documentation first.",
    produces: "One research document per task, with every external fact cited by URL and date.",
    protects: "Building on recalled knowledge of a library or API instead of its current documentation.",
  },
  {
    title: "Spec",
    why: "One spec states what the change must do, as testable requirements with acceptance criteria. Every later stage traces back to it, so a missing requirement is caught here instead of in code.",
    produces:
      "One spec file: requirements in EARS form, acceptance criteria, contracts, and an External Evidence table.",
    protects: "Requirements that live only in someone's head and never reach the plan.",
  },
  {
    title: "Fact-check",
    why: "When you approve your own spec or plan, a wrong claim in it returns as broken code. Fact-check verifies every concrete claim against the project and the cited sources before you are asked to approve.",
    produces: "A PASS or FAIL verdict with findings.",
    protects: "Claims about the code or external services that are false and would break the implementation.",
  },
  {
    title: "Plan",
    why: "Without a design step the architecture drifts. The plan turns the spec into phases with dependencies, risks, and a test policy, and it waits for your approval before any code is written.",
    produces: "A master plan and one self-contained file per phase.",
    protects: "Architecture drift and a change that must land in five places landing in four.",
  },
  {
    title: "Build",
    why: "Each phase is implemented by an agent that loads the project's skills and conventions and verifies every step with the project's build command, one phase at a time per project.",
    produces: "The code for one phase and a change report.",
    protects: "Unverified code. Every step runs the project's build command.",
  },
  {
    title: "Review",
    why: "You cannot review code you wrote an hour ago with fresh eyes. The reviewer checks every change against the project's rules and the phase's requirements, and the loop repeats until the findings are fixed.",
    produces: "Findings against the project's rule set and the phase's requirements, plus the review ledger.",
    protects: "Defects that compound across phases, and code that follows every convention but does the wrong thing.",
  },
  {
    title: "Test",
    why: "Tests are the first thing dropped under pressure. When you ask for them, every branch of the changed code is listed first, then each path gets a test. Which phases are skipped was decided in writing at planning time.",
    produces: "One test per execution path, following the project's test skills.",
    protects: "Testing as an afterthought. Tests run after every phase, so no later phase changes the code under test.",
  },
  {
    title: "Docs",
    why: "Documentation debt grows when nobody writes down how a change works. When you ask for it, the area references are refreshed from what actually changed.",
    produces: "Updated area references grounded in the real source.",
    protects: "The how-it-works knowledge that is gone six months later.",
  },
  {
    title: "Done",
    why: "The completion report names every stage that did not run and how to run it later, and under YOLO lists every decision Ostra made for you, so nothing is hidden.",
    produces: "What was done, what did not run and how to run it, and every decision made for you.",
    protects: "Silent omissions that read as bugs later.",
  },
];

type GateId = "spec" | "plan";

const GATES: Record<GateId, { kind: string; title: string; explanation: string; reason: string }> = {
  spec: {
    kind: "spec_approval",
    title: "Approve the spec",
    explanation: "Fact-check returned PASS on the spec. Your approval is recorded, then planning starts.",
    reason: "Fact-check PASS, no open questions.",
  },
  plan: {
    kind: "plan_approval",
    title: "Approve the plan",
    explanation: "Fact-check returned PASS on the plan. No code is written until you approve it.",
    reason: "Fact-check PASS.",
  },
};

// Scroll steps: a lane, or a gate that sits in the lane before it. Lane 8 is Done.
const STEPS: { lane: number; gate?: GateId }[] = [
  { lane: 0 },
  { lane: 1 },
  { lane: 2 },
  { lane: 2, gate: "spec" },
  { lane: 3 },
  { lane: 3, gate: "plan" },
  { lane: 4 },
  { lane: 5 },
  { lane: 6 },
  { lane: 7 },
  { lane: 8 },
];

type Approved = Record<GateId, boolean>;

type RingState = MarkState & { lane: number; tone: Tone; pulse: boolean; label: string; meta: string };

function stateAt(step: number, approved: Approved): RingState {
  if (step < 0)
    return {
      arcs: Array(MARK_ARCS).fill("off"),
      pearl: "off",
      lane: -1,
      tone: "neutral",
      pulse: false,
      label: "No session",
      meta: "8 stages · 2 gates",
    };
  const st = STEPS[Math.min(step, STEPS.length - 1)];
  if (st.lane >= MARK_ARCS)
    return {
      arcs: Array(MARK_ARCS).fill("done"),
      pearl: "ok",
      lane: MARK_ARCS,
      tone: "ok",
      pulse: false,
      label: "Completed",
      meta: "every gate passed",
    };
  const ok = st.gate ? approved[st.gate] : false;
  const wait = !!st.gate && !ok;
  const kind: MarkArc = wait ? "wait" : ok ? "done" : "run";
  return {
    arcs: NAMES.map((_, i) => (i < st.lane ? "done" : i === st.lane ? kind : "off")),
    pearl: wait ? "wait" : "run",
    lane: st.lane,
    tone: wait ? "warn" : "accent",
    pulse: !wait,
    label: wait ? "Waiting for you" : ok ? "Approved by you" : "Running",
    meta: wait
      ? `approve the ${st.gate}`
      : ok
        ? `${st.gate} approved`
        : `${NAMES[st.lane]} · stage ${st.lane + 1} of ${MARK_ARCS}`,
  };
}

const COLOR: Record<string, string> = {
  done: "var(--accent)",
  run: "var(--accent)",
  wait: "var(--warn)",
  fail: "var(--bad)",
  ok: "var(--ok)",
};

const C = 220;
const R = 150;
const pt = (r: number, deg: number) => {
  const t = (deg * Math.PI) / 180;
  return `${(C + r * Math.cos(t)).toFixed(2)} ${(C + r * Math.sin(t)).toFixed(2)}`;
};
const arcD = (i: number) => `M${pt(R, -90 + i * 45 + 2.2)}A${R} ${R} 0 0 1 ${pt(R, -90 + (i + 1) * 45 - 2.2)}`;

/** The mark's ring at section scale, with each stage's number and name beside its arc. */
function Ring({ state }: { state: RingState }) {
  const pearl = state.pearl && state.pearl !== "off" ? state.pearl : null;
  return (
    <svg viewBox="-60 0 580 440" width="100%" role="img" aria-label="Pipeline progress" className="home-ring">
      {state.arcs.map((k, i) => {
        const lit = k !== "off";
        const now = i === state.lane;
        const a = ((-67.5 + i * 45) * Math.PI) / 180;
        return (
          <g key={NAMES[i]}>
            <path d={arcD(i)} fill="none" stroke="var(--border-default)" strokeWidth={16} />
            <path
              d={arcD(i)}
              fill="none"
              stroke={COLOR[k] ?? COLOR.done}
              strokeWidth={16}
              pathLength={1}
              strokeDasharray="1 1"
              strokeDashoffset={lit ? 0 : 1}
              className={`home-ring__arc${k === "run" ? " live-mark-pulse" : ""}`}
            />
            <text
              x={C + 192 * Math.cos(a)}
              y={C + 192 * Math.sin(a)}
              textAnchor={Math.cos(a) > 0 ? "start" : "end"}
              dominantBaseline="middle"
              className="home-ring__label"
            >
              <tspan style={{ fill: now && lit ? (COLOR[k] ?? COLOR.done) : "var(--text-muted)" }}>
                {`${String(i + 1).padStart(2, "0")}  `}
              </tspan>
              <tspan
                style={{ fill: now ? "var(--text-primary)" : lit ? "var(--text-secondary)" : "var(--text-muted)" }}
              >
                {NAMES[i]}
              </tspan>
            </text>
          </g>
        );
      })}
      <circle
        cx={C}
        cy={C}
        r={26}
        fill={COLOR[pearl ?? "run"] ?? COLOR.run}
        className={`home-ring__pearl${pearl === "run" ? " live-mark-pulse" : ""}`}
        style={{ transform: pearl ? "scale(1)" : "scale(0)" }}
      />
    </svg>
  );
}

/**
 * The pipeline, one scroll step per stage and gate. The step whose top has crossed the middle of the viewport drives
 * the ring, and `onMark` reports the mark's state while the section is under the nav (null once it is not).
 */
export function Pipeline({ onMark }: { onMark: (m: MarkState | null) => void }) {
  const section = useRef<HTMLElement>(null);
  const [step, setStep] = useState(-1);
  const [inPipe, setInPipe] = useState(false);
  const [approved, setApproved] = useState<Approved>({ spec: false, plan: false });

  useEffect(() => {
    let raf = 0;
    const track = () => {
      raf = 0;
      const el = section.current;
      if (!el) return;
      const vh = window.innerHeight;
      let s = -1;
      for (const st of el.querySelectorAll<HTMLElement>("[data-step]"))
        if (st.getBoundingClientRect().top < vh * 0.5) s = Math.max(s, Number(st.dataset.step));
      const r = el.getBoundingClientRect();
      setStep(s);
      setInPipe(r.top < vh * 0.4 && r.bottom > vh * 0.4);
    };
    const onScroll = () => {
      if (!raf) raf = requestAnimationFrame(track);
    };
    track();
    window.addEventListener("scroll", onScroll, { passive: true });
    window.addEventListener("resize", onScroll);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onScroll);
    };
  }, []);

  const ring = stateAt(step, approved);
  useEffect(() => {
    const s = stateAt(step, approved);
    onMark(inPipe ? { arcs: s.arcs, pearl: s.pearl } : null);
  }, [step, inPipe, approved, onMark]);

  return (
    <section ref={section} className="home-section home-section--pipe">
      <div data-reveal className="home-section__head">
        <div className="site-eyebrow">Pipeline</div>
        <h2 className="home-h2">Eight stages, two gates</h2>
        <p className="home-lede">
          Every stage says what it produces and what it protects against. On the full track, Ostra stops at the spec and
          the plan, and nothing is built until you approve them.
        </p>
      </div>
      <div className="home-pipe">
        <div className="home-pipe__ringcol">
          <div className="home-pipe__sticky">
            <div className="home-pipe__ring">
              <Ring state={ring} />
            </div>
            <div className="home-pipe__status">
              <StatusDot tone={ring.tone} pulse={ring.pulse} />
              <span className="home-pipe__status-label">{ring.label}</span>
              <span className="home-pipe__status-meta">{ring.meta}</span>
            </div>
            <p className="home-muted home-pipe__caption">
              The mark is the pipeline: one arc per stage, clockwise from the top.
            </p>
          </div>
        </div>
        <div className="home-pipe__steps">
          {STEPS.map((st, idx) => {
            const key = st.gate ?? `lane-${st.lane}`;
            if (st.gate) {
              const g = GATES[st.gate];
              const done = approved[st.gate];
              const gate = st.gate;
              return (
                <div key={key} data-step={idx} data-reveal className="home-pipe__gate">
                  <GateCard
                    kind={g.kind}
                    title={g.title}
                    explanation={g.explanation}
                    answered={done}
                    answeredBy="user"
                    answer="Approved"
                    reason={g.reason}
                    actions={
                      done ? null : (
                        <Button variant="primary" onClick={() => setApproved((a) => ({ ...a, [gate]: true }))}>
                          {g.title}
                        </Button>
                      )
                    }
                  />
                </div>
              );
            }
            const lane = LANES[st.lane];
            const at = stateAt(idx, approved);
            const reached = idx <= step;
            const now = idx === step;
            const color = now && at.tone === "ok" ? "var(--ok)" : "var(--accent)";
            return (
              <div key={key} data-step={idx} data-reveal className="home-pipe__lane">
                <div
                  className="home-pipe__bar"
                  style={{ background: color, transform: reached ? "scaleX(1)" : "scaleX(0)" }}
                />
                <div className="home-pipe__title">
                  <div className="home-pipe__inline-mark">
                    <LiveMark state={at} size={24} />
                  </div>
                  {st.lane < MARK_ARCS && (
                    <span
                      className="home-pipe__num"
                      style={{ color: now ? color : reached ? "var(--text-secondary)" : "var(--text-muted)" }}
                    >
                      {String(st.lane + 1).padStart(2, "0")}
                    </span>
                  )}
                  <h3>{lane.title}</h3>
                </div>
                <p className="home-pipe__why">{lane.why}</p>
                <dl className="home-pipe__facts">
                  <dt>Produces</dt>
                  <dd>{lane.produces}</dd>
                  <dt>Protects against</dt>
                  <dd>{lane.protects}</dd>
                </dl>
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}

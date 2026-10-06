# Docs pipeline eval runs, 2026-10-06

These runs measure how the design of the docs stage changes the book that it writes. Each run wrote a book of
Ostra on one snapshot of this repository, and each book answers the same 221 questions in `questions/`.

## Setup

- **Snapshot.** The snapshot of 2026-09-30 that `book_area.json` was written on, with `docs/`, `HANDOVER.md`,
  `CLAUDE.md`, `README.md`, `site/`, and the earlier eval books removed, so that no writer could copy existing
  documentation.
- **Models.** Every docs subagent ran on Sonnet 5.5 at high effort: the documentation agent in each mode, the
  `explore` gatherers, and the `fact-check` agent. The judges ran on Sonnet 5.5.
- **Server.** A scratch server with its own data folder, and a `DOCS` request: "Write the documentation book for
  the ostra project."
- **Coverage.** A question counts as answered when a unit of the book states its answer, so that a reader of that
  unit alone can answer it. 12 to 16 labelers judged the top 10 search results of each question, and 2 more
  searched the whole book for each question that the first pass missed. The labels are in each run's labels
  folder, and `fullbook*.toml` holds the second pass.
- **Depth.** One grader scored each page from 0 (absent) to 2 (as deep as `docs/`) on the eight page questions of
  the prompt, compared four topics with `docs/`, and checked claims against the snapshot's code.

Run a corpus by its name: `OSTRA_EVAL_CORPUS=coverage cargo test -p ostra-core --test book_retrieval
retrieval_over_corpus -- --ignored --nocapture`.

## The runs

| Corpus | Design | Cost | Pages | Words |
| --- | --- | --- | --- | --- |
| `single` | One writer per project, with the page questions | $7.99 | 23 | 23,000 |
| `topics` | Plan, 8 `explore` gatherers, outline, 16 topic writers | $84.75 | 64 | 184,856 |
| `loop` | Survey, 25 page writers, 6 rounds of fact-checks and synthesis | $148.48 | 25 | 166,940 |
| `coverage` | The loop with the engine scan, coverage checks, and changed-part re-checks, 4 rounds | $115.48 | 30 | 222,323 |

For comparison, `docs/` has 35 pages and about 143,000 words, and `area` (the earlier area writers) has 172
sections.

## Coverage and ranking

| Corpus | Answered | hit@1 | hit@5 | MRR@10 |
| --- | --- | --- | --- | --- |
| `single` | 107 | 60.7% | 88.8% | 0.730 |
| `topics` | 211 | 64.9% | 89.6% | 0.756 |
| `loop` | 203 | 70.9% | 94.6% | 0.818 |
| `coverage` | 205 | 75.1% | 94.6% | 0.834 |
| `docs` (`docs_labels`) | 206 | 68.0% | 93.7% | 0.788 |
| `area` (`area_labels`) | 220 | 74.5% | 93.2% | 0.829 |

## Depth

| Question | `single` | `topics` | `loop` | `coverage` |
| --- | --- | --- | --- | --- |
| The problem | 1.09 | 1.75 | 1.84 | 2.00 |
| The mechanism | 1.74 | 1.94 | 2.00 | 1.97 |
| The reasons | 1.00 | 1.69 | 1.68 | 1.83 |
| The cases | 0.90 | 1.81 | 1.96 | 1.90 |
| The groups | 1.91 | 1.88 | 2.00 | 2.00 |
| The user's view | 0.91 | 1.31 | 1.60 | 1.37 |
| The limits | 1.76 | 1.94 | 2.00 | 1.93 |
| The code | 1.00 | 1.88 | 1.88 | 1.43 |

Accuracy: `single` 13 of 15 checked claims correct, `topics` 19 of 20, `loop` 36 of 38, and `coverage` 20 of 20.

## Findings

- **One writer cannot write a whole book.** Its result must fit in one reply, so `single` compressed each page to
  about 1,000 words and answered 107 questions.
- **Parallel writers without synthesis split the facts.** `topics` answered 211 questions, but it wrote 64 narrow
  pages, repeated the budget guard and the slot rules on 2 to 5 pages, and linked to pages that its outline later
  split.
- **The synthesis loop fixes the split.** `loop` and `coverage` wrote broad pages in groups, resolved every link,
  and kept each fact on one page with a few exceptions. They rank best: a question finds its answer first in 71%
  and 75% of cases.
- **LOW findings must not block done.** In `loop`, rounds 4 to 6 fixed only LOW fact-check findings and cost
  $26.70. Each revision drew new LOW findings, so the loop could not converge. `coverage` counts only HIGH and
  MEDIUM findings and ended by itself after 4 rounds.
- **Fact-checks cost the most.** In `loop`, the fact-checks cost $81.11 of $148.48 and the synthesis passes $1.42.
  A re-check that diffs a page against its previous draft cost about $0.19 a page in `coverage`, against about
  $1.04 for a full check.
- **The remaining gap is reasons and small facts.** `coverage` misses 16 questions: 6 ask for a reason that the
  removed `HANDOVER.md` states and the code does not, 7 ask for a behavior that no named constant or module check
  finds, such as the LSP column assumption, and 3 have an answer split across two `##` parts.

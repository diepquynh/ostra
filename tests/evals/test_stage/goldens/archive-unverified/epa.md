# Execution Path Analysis: Archive notes
**Date:** 2026-09-30 · **Implementer report:** {session}/ostra-implementer-phase-1.md · **Area(s):** notes, api, page · **Status:** Complete

## Summary
| # | Source File | Public Fns/Methods | Total Paths | New | Existing |
| - | ----------- | ------------------ | ----------- | --- | -------- |
| 1 | `src/notes.js` | 2 (`archiveNote`, `listNotes`) | 5 | 5 | 0 |
| 2 | `server/app.js` | 1 (`createApp`) | 3 | 3 | 0 |
| 3 | `public/app.js` | browser code | 0 | 0 | 0 |

**Flows:** 3 (2 new, verifiable) · **Regression suites:** 2 · **Unverified flows:** 1

## Detailed Analysis
The paths are those of the archive change: `archiveNote` (unknown id throws `NotFoundError`; an active note gets
`archived: true` and `archivedAt`; an archived note is returned unchanged) and `listNotes` (archived notes left
out by default, only archived ones with `{ archived: true }`), all at level unit.

## System Flows
| Flow | Description | Files Crossed (in order) | Entry Conditions | Key Assertions | Level | Status |
| ---- | ----------- | ------------------------ | ---------------- | -------------- | ----- | ------ |
| S1 | Archive through the API, then list | `server/app.js:L32-L34` → `src/notes.js:L17-L26` → `GET /api/notes` | a note made with `POST /api/notes` | 200 with `archived: true`; the default list leaves it out; an unknown id answers 404 | integration | NEW |
| S2 | `GET /api/notes?archived=true` | `server/app.js:L30` → `src/notes.js:L28-L32` | an archived note | only the archived note | integration | NEW |

## Regression Suites
| # | Test File or Group | Level | Command | Covers | Changed on Purpose |
| - | ------------------ | ----- | ------- | ------ | ------------------ |
| R1 | `test/unit/notes.test.js` | unit | `node --test test/unit/notes.test.js` | `createNote`, `listNotes` | No |
| R2 | `test/integration/api.test.js` | integration | `node --test test/integration/api.test.js` | the notes API and the page route | No |

## Unverified Flows
- S3, the Archive button and the "Show archived notes" checkbox in `public/app.js` (L10-L17, L23-L25, L41): the
  result is only visible in a browser, and this repo has no browser test type (only `unit` and `integration`). It
  needs an `e2e` level with a browser runner. It is not tested at a lower level, because a unit test of the page
  script with a fake `document` and `fetch` would replace every part the flow crosses.

## Test Writing Instructions
- Unit (`test/unit/archive.test.js`): the five `archiveNote` and `listNotes` paths.
- Integration (`test/integration/archive.test.js`): S1 and S2 with `withServer` and `post`.

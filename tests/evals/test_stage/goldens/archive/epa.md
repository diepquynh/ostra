# Execution Path Analysis: Archive notes
**Date:** 2026-09-30 · **Implementer report:** {session}/ostra-implementer-phase-1.md · **Area(s):** notes, api, page · **Status:** Complete

## Summary
| # | Source File | Public Fns/Methods | Total Paths | New | Existing |
| - | ----------- | ------------------ | ----------- | --- | -------- |
| 1 | `src/notes.js` | 2 (`archiveNote`, `listNotes`) | 5 | 5 | 0 |
| 2 | `server/app.js` | 1 (`createApp`: the archive route and `?archived`) | 3 | 3 | 0 |
| 3 | `public/app.js` | browser code (Archive button, Show archived) | 0 | 0 | 0 |

**Flows:** 3 (3 new) · **Regression suites:** 3 · **Unverified flows:** 0

## Detailed Analysis

### notes.js
**File:** `{repo}/src/notes.js` · **Area:** notes · **Test skills:** unit-test, convention

#### `archiveNote(store, id)`
| Path | Description | Entry Conditions | Key Assertions | Lines | Level | Status |
| ---- | ----------- | ---------------- | -------------- | ----- | ----- | ------ |
| P1 | Unknown id | `store.get(id)` is undefined | throws `NotFoundError` | L18-L21 | unit | NEW |
| P2 | Archive an active note | note not archived | returns the note with `archived: true` and `archivedAt` from `store.now()`; the store has it archived | L25 | unit | NEW |
| P3 | Archive an archived note | note already archived | returns it unchanged: `archivedAt` keeps the first value | L22-L24 | unit | NEW |

#### `listNotes(store, { archived = false })`
| Path | Description | Entry Conditions | Key Assertions | Lines | Level | Status |
| ---- | ----------- | ---------------- | -------------- | ----- | ----- | ------ |
| P4 | Default leaves archived notes out | no option | only unarchived notes, newest first | L28-L32 | unit | NEW |
| P5 | `archived: true` lists only archived notes | `{ archived: true }` | only archived notes | L28-L32 | unit | NEW |

### app.js (server)
**File:** `{repo}/server/app.js` · **Area:** api · **Test skills:** integration-test, convention
| Path | Description | Entry Conditions | Key Assertions | Lines | Level | Status |
| ---- | ----------- | ---------------- | -------------- | ----- | ----- | ------ |
| P6 | `POST /api/notes/:id/archive` | a note exists | 200 and the archived note | L32-L34 | integration (S1) | NEW |
| P7 | Unknown id on the archive route | no such note | 404 (NotFoundError mapped at L49-L50) | L32-L34 | integration (S1) | NEW |
| P8 | `GET /api/notes?archived=true` | an archived note | only archived notes | L30 | integration (S2) | NEW |

## System Flows
| Flow | Description | Files Crossed (in order) | Entry Conditions | Key Assertions | Level | Status |
| ---- | ----------- | ------------------------ | ---------------- | -------------- | ----- | ------ |
| S1 | Archive through the API, then list | `server/app.js:L32-L34` → `src/notes.js:L17-L26` → `src/store.js` → `GET /api/notes` (`server/app.js:L29-L30`, `src/notes.js:L28-L32`) | a note made with `POST /api/notes` | archive answers 200 with `archived: true`; the default list no longer has it; an unknown id answers 404 | integration | NEW |
| S2 | Show archived notes through the API | `GET /api/notes?archived=true` → `server/app.js:L30` → `src/notes.js:L28-L32` | an archived note | the list holds only the archived note | integration | NEW |
| S3 | Archive from the page | `public/app.js:L10-L17,L23-L25` (Archive button, `fetch` to the archive route, `refresh`) → S1 | the page shows a note | clicking Archive removes the note from the list; checking "Show archived notes" shows it again | e2e | NEW |

## Regression Suites
| # | Test File or Group | Level | Command | Covers | Changed on Purpose |
| - | ------------------ | ----- | ------- | ------ | ------------------ |
| R1 | `test/unit/notes.test.js` | unit | `node --test test/unit/notes.test.js` | `createNote`, `listNotes` (newest first) | No |
| R2 | `test/integration/api.test.js` | integration | `node --test test/integration/api.test.js` | `POST` and `GET /api/notes`, the page route | No |
| R3 | `e2e/notes.spec.js` | e2e | `npx playwright test e2e/notes.spec.js` | adding a note shows it in the list | Yes: each active note now carries an Archive button, so the list item reads `milk Archive`, not `milk` |

## Unverified Flows
None: the repo has an `e2e` test type for S3. It needs a browser installed, which write-test reports if it cannot run it.

## Test Writing Instructions
- P1 to P5: `test/unit/archive.test.js`, with `new MemoryStore(clock())` per test.
- S1, S2 (P6 to P8): `test/integration/archive.test.js`, with `withServer` and `post` from `./support.js`.
- S3: `e2e/archive.spec.js`: add a note, click Archive, expect no list items; check "Show archived notes", expect
  the note.
- R3: in `e2e/notes.spec.js`, the expected list item becomes `["milk Archive"]`.

# Execution Path Analysis: Totals in cents
**Date:** 2026-09-30 · **Implementer report:** {session}/ostra-implementer-phase-1.md · **Area(s):** http, client · **Status:** Complete

## Summary
The change replaces the order JSON's `total` string with `total_cents` (an integer) and `currency`, and moves the
formatting into the CLI client. It is a contract change, so the consumers matter as much as the new code.

| # | Source File | Public Fns/Methods | Total Paths | New | Existing |
| - | ----------- | ------------------ | ----------- | --- | -------- |
| 1 | `shop/api.py` | 1 (`order_json`) | 1 | 1 | 0 |
| 2 | `shop/cli.py` | 1 (`summary_line`) | 2 | 2 | 0 |

**Flows:** 2 (2 new) · **Regression suites:** 2 · **Unverified flows:** 0

## Detailed Analysis

### api.py
**File:** `{repo}/shop/api.py` · **Area:** http · **Test skills:** e2e-test, convention

#### `order_json(order) -> dict`
| Path | Description | Entry Conditions | Key Assertions | Lines | Level | Status |
| ---- | ----------- | ---------------- | -------------- | ----- | ----- | ------ |
| P1 | The JSON carries the total as cents and a currency, and no `total` | any order | `total_cents == order.total_cents()` (an int), `currency == "USD"`, `"total" not in` the JSON | L17-L28 | unit | NEW |

### cli.py
**File:** `{repo}/shop/cli.py` · **Area:** client · **Test skills:** unit-test, convention

#### `summary_line(order: dict) -> str`
| Path | Description | Entry Conditions | Key Assertions | Lines | Level | Status |
| ---- | ----------- | ---------------- | -------------- | ----- | ----- | ------ |
| P2 | One line, total formatted from cents | one line, `total_cents` 125 | `"Order 3: 1 line, $1.25, placed"` | L16-L19 | unit | NEW |
| P3 | Several lines | two lines, `total_cents` 340 | `"Order 4: 2 lines, $3.40, paid"` | L16-L19 | unit | NEW |

## System Flows
| Flow | Description | Files Crossed (in order) | Entry Conditions | Key Assertions | Level | Status |
| ---- | ----------- | ------------------------ | ---------------- | -------------- | ----- | ------ |
| S1 | `GET /orders/<id>` answers with `total_cents` and `currency` | `shop/server.py` → `shop/api.py:L17-L28` → `shop/domain.py` (`total_cents`) | an order placed with `POST /orders` | 200; `total_cents` is 2468 for 2 x 1234; `currency` is `USD`; no `total` key | e2e | NEW |
| S2 | The CLI reads the new field from the real API | `shop/cli.py:L9-L19` (`fetch_order`, `summary_line`) → `GET /orders/<id>` → `shop/api.py` | a server from `tests/e2e/support.py` with one placed order | `summary_line(fetch_order(url, id)) == "Order 1: 1 line, $24.68, placed"` | e2e | NEW |

## Regression Suites
| # | Test File or Group | Level | Command | Covers | Changed on Purpose |
| - | ------------------ | ----- | ------- | ------ | ------------------ |
| R1 | `tests/e2e/test_http_orders.py` | e2e | `python3 -m unittest tests.e2e.test_http_orders` | `test_place_then_show` asserts `created["total"] == "$24.68"` | Yes: the phase replaces `total` with `total_cents`, so the assertion becomes `created["total_cents"] == 2468` |
| R2 | `tests/unit/test_cli.py` | unit | `python3 -m unittest tests.unit.test_cli` | both tests pass `total` strings to `summary_line` | Yes: `summary_line` now reads `total_cents`, so the inputs become `total_cents: 125` and `340` with the same expected lines |

## Unverified Flows
None in this repository.

## Notes
- A consumer outside this repository reads the removed field: the storefront project,
  `{ws}/storefront/src/order-page.js`, `orderSummary` prints `order.total`. After this change it prints `undefined`.
  This analysis covers only `Repo root:`, so the storefront needs its own change and verification.

## Test Writing Instructions
- P1: in `tests/unit/test_api_json.py`, build an `Order` with lines (1234 x 2) and assert `order_json(order)` has
  `total_cents == 2468`, `currency == "USD"`, and no `total`.
- P2, P3: update `tests/unit/test_cli.py` (R2) to pass `total_cents` and keep the expected lines.
- S1, S2: in `tests/e2e/test_http_orders.py` (R1), assert `total_cents == 2468` and `currency == "USD"` instead of
  `total`, and add a test that runs `summary_line(fetch_order(url, id))` against `running_server()`.

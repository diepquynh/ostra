# Execution Path Analysis: Thousands separators
**Date:** 2026-09-30 · **Implementer report:** {session}/ostra-implementer-phase-1.md · **Area(s):** money · **Status:** Complete

## Summary
| # | Source File | Public Fns/Methods | Total Paths | New | Existing |
| - | ----------- | ------------------ | ----------- | --- | -------- |
| 1 | `shop/money.py` | 1 (`format_money`) | 5 | 5 | 0 |

**Flows:** 0 new · **Regression suites:** 3 · **Unverified flows:** 0

## Detailed Analysis

### money.py
**File:** `{repo}/shop/money.py` · **Area:** money · **Test skills:** unit-test, convention

#### `format_money(cents: int) -> str`
| Path | Description | Entry Conditions | Key Assertions | Lines | Level | Status |
| ---- | ----------- | ---------------- | -------------- | ----- | ----- | ------ |
| P1 | Under a thousand dollars: no separator | `abs(cents) < 100000` | `format_money(99975) == "$999.75"` | L21 | unit | NEW |
| P2 | Thousands are separated | `100000 <= abs(cents) < 100000000` | `format_money(123325) == "$1,233.25"` | L21 (`{whole:,}`) | unit | NEW |
| P3 | Millions are separated | `abs(cents) >= 100000000` | `format_money(100000000) == "$1,000,000.00"` | L21 | unit | NEW |
| P4 | Negative with a separator | `cents <= -100000` | `format_money(-123456) == "-$1,234.56"` | L19-L21 | unit | NEW |
| P5 | Small negative amount | `-100 < cents < 0` | `format_money(-5) == "-$0.05"` | L19-L20 | unit | NEW |

## System Flows
No new flow: the callers are unchanged. `invoice_lines` and `order_json` (`GET /orders/<id>`, the `total` field)
now show separators for totals of $1,000 and more; the regression suites below cover them.

## Regression Suites
| # | Test File or Group | Level | Command | Covers | Changed on Purpose |
| - | ------------------ | ----- | ------- | ------ | ------------------ |
| R1 | `tests/unit/test_invoice.py` | unit | `python3 -m unittest tests.unit.test_invoice` | `invoice_lines` prints `Total: $1233.25` for a $1,233.25 order | Yes: the change is to show thousands separators, so the expected total line becomes `Total: $1,233.25` |
| R2 | `tests/e2e/test_http_orders.py` | e2e | `python3 -m unittest tests.e2e.test_http_orders` | `order_json` formats `total` ($24.68, no separator) | No |
| R3 | `tests/unit/test_cli.py` | unit | `python3 -m unittest tests.unit.test_cli` | `summary_line` prints the `total` it is given | No |

## Unverified Flows
None.

## Test Writing Instructions
Write `tests/unit/test_format_money.py`, class `FormatMoneyTest`, following the unit-test skill:
- P1: `format_money(99975) == "$999.75"`.
- P2: `format_money(123325) == "$1,233.25"`.
- P3: `format_money(100000000) == "$1,000,000.00"`.
- P4: `format_money(-123456) == "-$1,234.56"`.
- P5: `format_money(-5) == "-$0.05"`.

Update R1: in `tests/unit/test_invoice.py`, the expected last line becomes `"Total: $1,233.25"`; the other lines stay.

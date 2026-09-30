# Execution Path Analysis: format_money
**Date:** 2026-09-30 · **Implementer report:** {session}/ostra-test-request.md (a test request) · **Area(s):** money · **Status:** Complete

## Summary
The implementer report is a test request, so the files come from the request: it names `format_money` in
`shop/money.py`, which has no tests of its own. The code is unchanged; every path is NEW because no test calls
`format_money` directly.

| # | Source File | Public Fns/Methods | Total Paths | New | Existing |
| - | ----------- | ------------------ | ----------- | --- | -------- |
| 1 | `shop/money.py` | 1 (`format_money`) | 6 | 6 | 0 |

**Flows:** 0 · **Regression suites:** 3 · **Unverified flows:** 0

## Detailed Analysis

### money.py
**File:** `{repo}/shop/money.py` · **Area:** money · **Test skills:** unit-test, convention
**Dependencies:** none.

#### `format_money(cents: int) -> str`
| Path | Description | Entry Conditions | Key Assertions | Lines | Level | Status |
| ---- | ----------- | ---------------- | -------------- | ----- | ----- | ------ |
| P1 | Dollars and cents | `cents >= 100`, two-digit cents | `format_money(1234) == "$12.34"` | L19-L21 | unit | NEW |
| P2 | Cents below ten are padded | `cents % 100 < 10` | `format_money(1205) == "$12.05"` | L21 (`{frac:02d}`) | unit | NEW |
| P3 | Zero | `cents == 0` | `format_money(0) == "$0.00"` | L19-L21 | unit | NEW |
| P4 | Less than a dollar | `0 < cents < 100` | `format_money(5) == "$0.05"` | L20 | unit | NEW |
| P5 | Negative: the minus goes before the dollar sign | `cents < 0` | `format_money(-5) == "-$0.05"` | L19 | unit | NEW |
| P6 | Negative dollars use the absolute value | `cents <= -100` | `format_money(-123456) == "-$1234.56"` | L20 (`abs`) | unit | NEW |

## System Flows
None new. `format_money` is unchanged, and the flows that use it (`invoice_lines`, `order_json` in `shop/api.py`,
and through it `GET /orders/<id>`) are already covered by the suites below.

## Regression Suites
| # | Test File or Group | Level | Command | Covers | Changed on Purpose |
| - | ------------------ | ----- | ------- | ------ | ------------------ |
| R1 | `tests/unit/test_invoice.py` | unit | `python3 -m unittest tests.unit.test_invoice` | `invoice_lines` calls `format_money` | No |
| R2 | `tests/e2e/test_http_orders.py` | e2e | `python3 -m unittest tests.e2e.test_http_orders` | `order_json` formats `total` | No |
| R3 | `tests/unit/test_money.py` | unit | `python3 -m unittest tests.unit.test_money` | `to_cents`, in the same module | No |

## Unverified Flows
None.

## Test Writing Instructions
Write `tests/unit/test_format_money.py`, class `FormatMoneyTest`, one test per path, following the unit-test skill.

- P1: `test_dollars_and_cents`: `format_money(1234) == "$12.34"`.
- P2: `test_pads_cents_below_ten`: `format_money(1205) == "$12.05"`.
- P3: `test_zero`: `format_money(0) == "$0.00"`.
- P4: `test_less_than_a_dollar`: `format_money(5) == "$0.05"`.
- P5: `test_negative_amount_puts_the_minus_before_the_dollar_sign`: `format_money(-5) == "-$0.05"`.
- P6: `test_negative_dollars`: `format_money(-123456) == "-$1234.56"`.

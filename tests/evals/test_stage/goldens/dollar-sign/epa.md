# Execution Path Analysis: Dollar sign in amounts
**Date:** 2026-09-30 · **Implementer report:** {session}/ostra-implementer-phase-1.md · **Area(s):** money · **Status:** Complete

## Summary
| # | Source File | Public Fns/Methods | Total Paths | New | Existing |
| - | ----------- | ------------------ | ----------- | --- | -------- |
| 1 | `shop/money.py` | 1 (`to_cents`) | 7 | 2 | 5 |

**Flows:** 0 · **Regression suites:** 1 · **Unverified flows:** 0

## Detailed Analysis

### money.py
**File:** `{repo}/shop/money.py` · **Area:** money · **Test skills:** unit-test, convention

#### `to_cents(amount: str) -> int`
| Path | Description | Entry Conditions | Key Assertions | Lines | Level | Status |
| ---- | ----------- | ---------------- | -------------- | ----- | ----- | ------ |
| P1 | A leading dollar sign is accepted | text starts with `$` | `to_cents("$12.34") == 1234` | L10 | unit | NEW |
| P2 | Minus before the dollar sign | text starts with `-$` | `to_cents("-$0.50") == -50` | L8-L10 | unit | NEW |
| P3 | Dollars and cents | two decimal digits | `to_cents("12.34") == 1234` | L11-L14 | unit | EXISTING |
| P4 | One decimal digit is tens of cents | one decimal digit | `to_cents("12.3") == 1230` | L14 | unit | EXISTING |
| P5 | Whole dollars | no decimal point | `to_cents("12") == 1200` | L14 | unit | EXISTING |
| P6 | Negative amount | leading `-` | `to_cents("-0.50") == -50` | L8-L9 | unit | EXISTING |
| P7 | Rejects bad input | three decimal digits or text | raises `ValueError` | L12-L13 | unit | EXISTING |

## System Flows
None: `to_cents` has no caller in `shop/`.

## Regression Suites
| # | Test File or Group | Level | Command | Covers | Changed on Purpose |
| - | ------------------ | ----- | ------- | ------ | ------------------ |
| R1 | `tests/unit/test_money.py` | unit | `python3 -m unittest tests.unit.test_money` | every existing `to_cents` path, P3 to P7 | No: the change adds the dollar sign and must keep every existing result |

## Unverified Flows
None.

## Test Writing Instructions
Add to `tests/unit/test_money.py`, class `ToCentsTest`, following the unit-test skill:
- P1: `test_leading_dollar_sign`: `to_cents("$12.34") == 1234`.
- P2: `test_minus_before_the_dollar_sign`: `to_cents("-$0.50") == -50`.
Then run R1 unchanged.

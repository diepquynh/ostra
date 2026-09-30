# Test Report: Dollar sign in amounts
**Date:** 2026-09-30 · **Implementer report:** {session}/ostra-implementer-phase-1.md · **Areas:** money · **Status:** Stuck: Escalation Required

## Changes Made
None.

## Escalation Request
| Field | Value |
| ----- | ----- |
| **Trigger** | Implementation bug discovered |
| **Stuck at file** | R1, `tests/unit/test_money.py` |
| **Attempts made** | 1: ran the regression suite before writing tests |
| **Error message** | `FAIL: test_one_decimal_digit_is_tens_of_cents` `AssertionError: 1203 != 1230` |
| **What I need** | An implementer fix in `shop/money.py`: `int(frac or "0")` reads "12.3" as 1203 cents; the fraction must be padded to two digits again, as before the change |
| **Tests completed so far** | none |

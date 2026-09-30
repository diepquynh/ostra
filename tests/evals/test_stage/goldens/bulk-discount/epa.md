# Execution Path Analysis: Bulk discount
**Date:** 2026-09-30 · **Implementer report:** {session}/ostra-implementer-phase-1.md · **Area(s):** pricing · **Status:** Complete

## Summary
| # | Source File | Public Fns/Methods | Total Paths | New | Existing |
| - | ----------- | ------------------ | ----------- | --- | -------- |
| 1 | `shop/pricing.py` | 1 (`bulk_discount`; `line_total` unchanged) | 8 | 8 | 0 |

**Flows:** 0 · **Regression suites:** 1 · **Unverified flows:** 0

## Detailed Analysis

### pricing.py
**File:** `{repo}/shop/pricing.py` · **Area:** pricing · **Test skills:** unit-test, convention
**Dependencies:** `BULK_TIERS` = ((50, 10), (10, 5)), checked largest first.

#### `bulk_discount(quantity: int, unit_cents: int) -> int`
| Path | Description | Entry Conditions | Key Assertions | Lines | Level | Status |
| ---- | ----------- | ---------------- | -------------- | ----- | ----- | ------ |
| P1 | Zero quantity is refused | `quantity == 0` | raises `ValueError` | L16-L17 | unit | NEW |
| P2 | Negative quantity is refused | `quantity < 0` | raises `ValueError` | L16-L17 | unit | NEW |
| P3 | Negative unit price is refused | `quantity > 0`, `unit_cents < 0` | raises `ValueError` | L18-L19 | unit | NEW |
| P4 | 10% from 50 units | `quantity >= 50` (boundary 50) | `bulk_discount(50, 1000) == 5000` | L20-L22 | unit | NEW |
| P5 | 5% from 10 to 49 units | `10 <= quantity < 50` (boundaries 10 and 49) | `bulk_discount(10, 1000) == 500`, `bulk_discount(49, 1000) == 2450` | L20-L22 | unit | NEW |
| P6 | No discount below 10 units | `0 < quantity < 10` (boundary 9) | `bulk_discount(9, 1000) == 0` | L23 | unit | NEW |
| P7 | The discount rounds down to whole cents | a tier applies and the product is not a multiple of 100 | `bulk_discount(10, 99) == 49` (49.5 rounded down) | L22 | unit | NEW |
| P8 | A free item has no discount | `unit_cents == 0`, any tier | `bulk_discount(50, 0) == 0` | L22 | unit | NEW |

**Delegated helpers:** none.

## System Flows
None. Nothing calls `bulk_discount` yet: `line_total` and `Order.total_cents` do not use it, so no route, command,
or consumer reaches the new code.

## Regression Suites
| # | Test File or Group | Level | Command | Covers | Changed on Purpose |
| - | ------------------ | ----- | ------- | ------ | ------------------ |
| R1 | `tests/unit/test_pricing.py` | unit | `python3 -m unittest tests.unit.test_pricing` | `line_total`, in the changed module | No |

## Unverified Flows
None.

## Test Writing Instructions
For each NEW path, write-test creates one test in `tests/unit/test_bulk_discount.py`, class `BulkDiscountTest`,
following the unit-test skill. No doubles are needed: the function is pure.

- P1: `test_rejects_a_zero_quantity`: `bulk_discount(0, 1000)` raises `ValueError`.
- P2: `test_rejects_a_negative_quantity`: `bulk_discount(-1, 1000)` raises `ValueError`.
- P3: `test_rejects_a_negative_unit_price`: `bulk_discount(10, -1)` raises `ValueError`.
- P4: `test_ten_percent_from_fifty_units`: `bulk_discount(50, 1000) == 5000`.
- P5: `test_five_percent_from_ten_units` (`bulk_discount(10, 1000) == 500`) and
  `test_five_percent_up_to_forty_nine_units` (`bulk_discount(49, 1000) == 2450`).
- P6: `test_no_discount_below_ten_units`: `bulk_discount(9, 1000) == 0`.
- P7: `test_rounds_down_to_whole_cents`: `bulk_discount(10, 99) == 49`.
- P8: `test_a_free_item_has_no_discount`: `bulk_discount(50, 0) == 0`.

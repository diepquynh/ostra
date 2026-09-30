# Execution Path Analysis: Order cancellation
**Date:** 2026-09-30 · **Implementer report:** {session}/ostra-implementer-phase-1.md · **Area(s):** orders, http · **Status:** Complete

## Summary
| # | Source File | Public Fns/Methods | Total Paths | New | Existing |
| - | ----------- | ------------------ | ----------- | --- | -------- |
| 1 | `shop/service.py` | 1 (`OrderService.cancel`) | 5 | 5 | 0 |
| 2 | `shop/api.py` | 1 (`Api.cancel`, and its route in `ROUTES`) | 1 | 1 | 0 |
| 3 | `shop/domain.py` | 0 (adds the `CANCELLED` status) | 0 | 0 | 0 |

**Flows:** 1 (1 new) · **Regression suites:** 3 · **Unverified flows:** 0

## Detailed Analysis

### service.py
**File:** `{repo}/shop/service.py` · **Area:** orders · **Test skills:** unit-test, convention
**Dependencies:** `repo: OrderRepository` (`get`, `set_status`).

#### `OrderService.cancel(order_id: int) -> Order`
| Path | Description | Entry Conditions | Key Assertions | Lines | Level | Status |
| ---- | ----------- | ---------------- | -------------- | ----- | ----- | ------ |
| P1 | Unknown order | `repo.get` returns `None` | raises `NotFound`; `set_status` not called | L35 (via `get`, L22-L25) | unit | NEW |
| P2 | Already cancelled | status is `cancelled` | raises `InvalidState` ("already cancelled"); `set_status` not called | L36-L37 | unit | NEW |
| P3 | Shipped | status is `shipped` | raises `InvalidState` ("has shipped"); `set_status` not called | L38-L39 | unit | NEW |
| P4 | Placed order is cancelled | status is `placed` | returns the order with status `cancelled`; `repo.set_status(order_id, "cancelled")` called once | L40-L42 | unit | NEW |
| P5 | Paid order is cancelled | status is `paid` | same as P4 | L40-L42 | unit | NEW |

### api.py
**File:** `{repo}/shop/api.py` · **Area:** http · **Test skills:** e2e-test, convention

#### `Api.cancel(order_id: str, body: bytes)` and `("POST", r"^/orders/(\d+)/cancel$", "cancel")` in `ROUTES`
| Path | Description | Entry Conditions | Key Assertions | Lines | Level | Status |
| ---- | ----------- | ---------------- | -------------- | ----- | ----- | ------ |
| P6 | The route returns the cancelled order | `POST /orders/<id>/cancel` | status 200 and the order JSON | L14, L71-L72 | e2e (S1) | NEW |

**Delegated helpers:** `Api.handle` (L32-L48) maps `NotFound` to 404, `InvalidState` to 409, `ValueError` to 400,
and an unmatched path to 404 ("no route").

## System Flows
| Flow | Description | Files Crossed (in order) | Entry Conditions | Key Assertions | Level | Status |
| ---- | ----------- | ------------------------ | ---------------- | -------------- | ----- | ------ |
| S1 | `POST /orders/<id>/cancel` cancels through the service and stores the status | `shop/server.py` → `shop/api.py:L14,L32-L48,L71-L72` → `shop/service.py:L33-L42` → `shop/repository.py` (`set_status`) | a placed or paid order made through `POST /orders` | 200 with `status: "cancelled"`; a later `GET /orders/<id>` shows `cancelled`; a shipped order answers 409 and stays `shipped`; a second cancel answers 409; an unknown id answers 404 | e2e | NEW |

## Regression Suites
| # | Test File or Group | Level | Command | Covers | Changed on Purpose |
| - | ------------------ | ----- | ------- | ------ | ------------------ |
| R1 | `tests/e2e/test_http_orders.py` | e2e | `python3 -m unittest tests.e2e.test_http_orders` | the other routes in `ROUTES` and `Api.handle` | No |
| R2 | `tests/integration/test_service.py` | integration | `python3 -m unittest tests.integration.test_service` | `OrderService` place, pay, ship | No |
| R3 | `tests/integration/test_repository.py` | integration | `python3 -m unittest tests.integration.test_repository` | `set_status`, which cancel calls | No |

## Unverified Flows
None.

## Test Writing Instructions

### service.py tests (`tests/unit/test_service_cancel.py`, unit)
Build `OrderService` on `Mock(spec=OrderRepository)` with `get.return_value` set to an `Order` in the needed status
(id 1), per the unit-test skill.
- P1: `get.return_value = None`; `cancel(1)` raises `NotFound`.
- P2: status `cancelled`; raises `InvalidState`; `set_status.assert_not_called()`.
- P3: status `shipped`; raises `InvalidState`; `set_status.assert_not_called()`.
- P4: status `placed`; returned status is `cancelled`; `set_status.assert_called_once_with(1, "cancelled")`.
- P5: status `paid`; same assertions as P4.

### Flow tests (`tests/e2e/test_http_cancel.py`, e2e)
Use `running_server()` and `call()` from `tests/e2e/support.py`; make each order with `POST /orders`.
- S1a: cancel a placed order: 200, `status == "cancelled"`, and `GET` shows `cancelled`.
- S1b: pay, then cancel: 200, `cancelled`.
- S1c: pay, ship, then cancel: 409 with an error naming the shipped order; `GET` still shows `shipped`.
- S1d: cancel twice: the second answers 409 ("already cancelled").
- S1e: `POST /orders/999/cancel` answers 404.

# Execution Path Analysis: Order lifecycle over HTTP
**Date:** 2026-09-30 · **Implementer report:** {session}/ostra-test-request.md (a test request) · **Area(s):** http, orders · **Status:** Complete

## Summary
The implementer report is a test request that names a flow, not files: placing, paying, shipping, and
cancelling an order over HTTP, with the error answers. Its entry points are the routes in `shop/api.py`
(`ROUTES`, L9-L15), which `Api.handle` dispatches to `OrderService`. No code changes; the checks are new
end-to-end tests of those routes.

| # | Source File | Public Fns/Methods | Total Paths | New | Existing |
| - | ----------- | ------------------ | ----------- | --- | -------- |
| 1 | `shop/api.py` | `Api.handle`, `create`, `show`, `pay`, `ship`, `cancel` | 10 | 7 | 3 |

**Flows:** 10 (7 new) · **Regression suites:** 3 · **Unverified flows:** 0

## System Flows
| Flow | Description | Files Crossed (in order) | Entry Conditions | Key Assertions | Level | Status |
| ---- | ----------- | ------------------------ | ---------------- | -------------- | ----- | ------ |
| S1 | Place and show | `POST /orders`, `GET /orders/<id>` → `shop/api.py` → `shop/service.py` → `shop/repository.py` | a valid order | 201, then 200 with the same order | e2e | EXISTING |
| S2 | Pay | `POST /orders/<id>/pay` | a placed order | 200, `paid` | e2e | EXISTING |
| S3 | Unknown order | `GET /orders/<id>` | no such id | 404 | e2e | EXISTING |
| S4 | Ship a paid order | `POST /orders/<id>/ship` | a paid order | 200, `shipped`; `GET` shows `shipped` | e2e | NEW |
| S5 | Cancel a placed or paid order | `POST /orders/<id>/cancel` | placed, or placed then paid | 200, `cancelled`; `GET` shows `cancelled` | e2e | NEW |
| S6 | Ship before paying | `POST /orders/<id>/ship` | a placed order | 409 (`InvalidState` mapped at L44-L45); the order stays `placed` | e2e | NEW |
| S7 | Pay twice | `POST /orders/<id>/pay` twice | a paid order | 409 | e2e | NEW |
| S8 | Cancel a shipped order, or cancel twice | `POST /orders/<id>/cancel` | shipped, or already cancelled | 409 | e2e | NEW |
| S9 | An invalid order | `POST /orders` with no lines, or no customer | bad payload | 400 (`ValueError` mapped at L46-L47) | e2e | NEW |
| S10 | Pay an unknown order | `POST /orders/999/pay` | no such id | 404 | e2e | NEW |

## Regression Suites
| # | Test File or Group | Level | Command | Covers | Changed on Purpose |
| - | ------------------ | ----- | ------- | ------ | ------------------ |
| R1 | `tests/e2e/test_http_orders.py` | e2e | `python3 -m unittest tests.e2e.test_http_orders` | S1 to S3 | No |
| R2 | `tests/integration/test_service.py` | integration | `python3 -m unittest tests.integration.test_service` | the service rules behind the routes | No |
| R3 | `tests/integration/test_repository.py` | integration | `python3 -m unittest tests.integration.test_repository` | the stored status | No |

## Unverified Flows
None.

## Test Writing Instructions
Write `tests/e2e/test_http_lifecycle.py` with `running_server()` and `call()`, one test per NEW flow (S4 to S10),
making every order through `POST /orders`. Assert the status code and, for success, the `status` field and a
following `GET`.

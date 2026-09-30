---
name: convention
description: How code in shop is written. Load for any change to shop/ or tests/.
---

# shop conventions

- Python 3.12, standard library only. Never add a dependency.
- Amounts are integer cents. Only `shop.money.format_money` turns cents into text, and only `api`, `cli`, and
  `invoice` show text amounts.
- Order rules live in `OrderService` and raise `NotFound`, `InvalidState`, or `ValueError`. `Api.handle` maps
  them to 404, 409, and 400; nothing else catches them.
- Type hints on every public function. No comments that repeat the code.
- Test modules are named `test_<module>.py` and test methods `test_<behavior>`, so a failing name says what broke.

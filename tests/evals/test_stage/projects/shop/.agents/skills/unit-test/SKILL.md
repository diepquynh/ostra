---
name: unit-test
description: Write a unit test under tests/unit/ for one function or class of shop.
---

# Unit tests

1. Put the test in `tests/unit/test_<module>.py`, as a `unittest.TestCase` subclass named after the unit.
2. One behavior per test method. Name it `test_<behavior>` or `test_<behavior>_when_<condition>`.
3. No files, sockets, databases, or clock reads. Replace collaborators with `unittest.mock.Mock(spec=...)`; for
   `OrderService`, pass a `Mock(spec=OrderRepository)` and set `get.return_value` to the order the test needs.
4. Assert the result: the return value with `assertEqual`, a raised error with `assertRaises` and the exact
   type, and an interaction with `assert_called_once_with` when the interaction is the result (a stored status).
5. Run one module with `python3 -m unittest tests.unit.test_<module>`.

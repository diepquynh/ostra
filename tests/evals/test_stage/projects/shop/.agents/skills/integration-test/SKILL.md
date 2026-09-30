---
name: integration-test
description: Write an integration test under tests/integration/ that runs the real service and repository over SQLite.
---

# Integration tests

1. Put the test in `tests/integration/test_<module>.py`.
2. In `setUp`, create a `tempfile.TemporaryDirectory()` and an `OrderRepository` on a `shop.db` file inside it;
   build the real `OrderService` (and `Api`, when the test covers a route) on it. In `tearDown`, close the
   repository and clean up the directory.
3. Never mock the parts under test. The point of this level is the real wiring and the real SQL.
4. Check stored state by reading it back through a fresh `repo.get(...)`, not only through a returned object.
5. Run one module with `python3 -m unittest tests.integration.test_<module>`.

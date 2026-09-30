---
name: unit-test
description: Write a unit test under test/unit/ for a function in src/.
---

# Unit tests

1. Put the test in `test/unit/<area>.test.js`. Import `test` from `node:test` and `assert` from `node:assert/strict`.
2. Build a `new MemoryStore(clock())` per test, where `clock()` returns a counter (`let t = 0; return () => ++t;`),
   so creation order is deterministic.
3. One behavior per `test(...)`, named as a sentence about the behavior.
4. Assert results with `assert.equal`, `assert.deepEqual`, and `assert.throws(fn, ErrorClass)` with the exact class.
5. Run one file with `node --test test/unit/<area>.test.js`.

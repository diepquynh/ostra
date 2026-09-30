---
name: e2e-test
description: Write an end-to-end test under tests/e2e/ that drives the real HTTP server.
---

# End-to-end tests

1. Put the test in `tests/e2e/test_http_<area>.py`.
2. Start the server with `tests.e2e.support.running_server()`, which yields the base URL of a server on a fresh
   database and stops it afterwards. Start one server per test method.
3. Send requests with `tests.e2e.support.call(url, method, path, payload)`, which returns the status code and
   the decoded JSON body, for errors too.
4. Assert the status code and the parts of the body the behavior is about. Set up state through the API itself
   (place, then pay), never by writing to the database.
5. Run one module with `python3 -m unittest tests.e2e.test_http_<area>`.

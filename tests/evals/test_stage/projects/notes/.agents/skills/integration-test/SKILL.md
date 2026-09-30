---
name: integration-test
description: Write an integration test under test/integration/ that calls the real app over HTTP.
---

# Integration tests

1. Put the test in `test/integration/<area>.test.js`.
2. Wrap each test in `withServer(async (url) => ...)` from `./support.js`, which starts the real app with a fresh
   store on a free port and closes it afterwards. Send POST requests with `post(url, body)`.
3. Never replace the store or the rules with doubles: this level checks the routes, the JSON, and the rules together.
4. Assert the status code and the JSON body. Set up state through the API itself.
5. Run one file with `node --test test/integration/<area>.test.js`.

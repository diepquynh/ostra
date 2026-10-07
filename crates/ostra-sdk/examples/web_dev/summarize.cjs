// Prints a short summary of a Playwright JSON report, because the whole report can outgrow a tool reply.
const fs = require("node:fs");

const report = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const plain = (s) => String(s ?? "").replace(/\u001b\[[0-9;]*m/g, "");
const failed = [];
const passed = [];
let skipped = 0;

const walk = (suite, titles) => {
  const here = suite.title && suite.title !== suite.file ? [...titles, suite.title] : titles;
  for (const spec of suite.specs ?? []) {
    const tests = spec.tests ?? [];
    const name = [...here, spec.title].join(" > ");
    const file = `${spec.file}:${spec.line}`;
    if (tests.length > 0 && tests.every((t) => t.status === "skipped")) {
      skipped += 1;
    } else if (spec.ok) {
      passed.push({ name, file, status: "pass" });
    } else {
      const error = tests
        .flatMap((t) => t.results ?? [])
        .map((r) => r.error?.message)
        .find(Boolean);
      failed.push({ name, file, status: "fail", error: plain(error).slice(0, 600) });
    }
  }
  for (const child of suite.suites ?? []) walk(child, here);
};
for (const suite of report.suites ?? []) walk(suite, []);

process.stdout.write(
  JSON.stringify({
    failed: failed.slice(0, 30),
    failed_count: failed.length,
    passed: passed.slice(0, 100),
    passed_count: passed.length,
    skipped_count: skipped,
    errors: (report.errors ?? []).map((e) => plain(e.message).slice(0, 600)).slice(0, 5),
  }),
);

# xAI

Last checked: 2026-09-26. Check the sources at the end of this page again before a release. xAI's legal pages
now show the name SpaceXAI.

## Native executor

Ostra has no native xAI provider. The native registry builds only `anthropic` and `openai`. It ignores all other
names in `[providers]` (`Providers::rebuild` in `crates/ostra-providers/src/lib.rs`). To use Grok models, route
agents to the Grok Build harness.

## Harness executor: Grok Build

Route an agent to `harness:grok`. Then sign in to Grok Build with `grok login`.

xAI says these things about Grok Build ([announcement](https://x.ai/news/grok-build-cli)):

- It is open to "all SuperGrok and X Premium Plus subscribers."
- It has a headless `-p` mode that "allows easily running agents inside scripts and automations."
- It offers "full ACP support to build your own bots and agent orchestration apps."

Thus, xAI advertises Grok Build for use by orchestration tools. Ostra uses Grok Build as a CLI. It runs xAI's own
binary, and that binary signs in through xAI's own flow. Ostra never uses xAI's OAuth client.

These questions are still open:

- xAI updated the consumer terms on 2026-09-11. Ostra could not fetch them on 2026-09-26. The December 2024
  version forbade automated access that "sends more request messages to the servers running the Service than
  a human can reasonably produce" in the same time with a web browser. Keep `limits.max_parallel_executions`
  low on a subscription.
- xAI's enterprise terms control API use, that is, use with an xAI API key that xAI bills per token. Reports
  say that Grok Build accepts an API key for headless work. But xAI's announcement does not document it. Read
  the current Grok Build docs before you rely on it.
- xAI approved subscription sign-in for some named tools: OpenCode, Hermes Agent, OpenClaw, Kilo, and Warp.
  xAI has not published a rule for arbitrary local apps. Ostra does not need that approval, because it never
  touches the subscription sign-in. But xAI has not confirmed this reading.

### Credentials

Ostra runs each Grok execution with its own `GROK_HOME`. That directory links the entries of your real
`GROK_HOME`, and `auth.json` is one of them. Thus, Grok reads its own sign-in (`plan_grok` in
`crates/ostra-exec-harness/src/launch.rs`). In the per-execution copy, Ostra writes new versions of only `config.toml`
and `trusted_folders.toml`. Ostra never opens `auth.json`. It only checks that the file exists, to show the login
status.

## Sources

- [Introducing Grok Build](https://x.ai/news/grok-build-cli)
- [SpaceXAI consumer terms of service](https://x.ai/legal/terms-of-service)
- [SpaceXAI enterprise terms of service](https://x.ai/legal/terms-of-service-enterprise)
- [xAI consumer terms, December 2024 version](https://x.ai/legal/terms-of-service/previous-2024-12-20)
- Reports about Grok Build plans and third-party sign-in: [OpenClaw xAI docs](https://docs.openclaw.ai/providers/xai),
  [Hermes Agent guide](https://hermes-agent.nousresearch.com/docs/guides/xai-grok-oauth),
  [Yoetz issue #405](https://github.com/TheGaySupreme123/yoetz/issues/405)

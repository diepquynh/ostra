# xAI

Last checked: 2026-09-26. Recheck the sources at the end of this page before a release. xAI's legal pages now
appear under the name SpaceXAI.

## Native executor

Ostra has no native xAI provider. The native registry builds only `anthropic` and `openai`, and ignores any
other name in `[providers]` (`Providers::rebuild` in `crates/ostra-providers/src/lib.rs`). To use Grok models,
route agents to the Grok Build harness.

## Harness executor: Grok Build

Route an agent to `harness:grok` and sign Grok Build in with `grok login`.

What xAI says about Grok Build ([announcement](https://x.ai/news/grok-build-cli)):

- It is open to "all SuperGrok and X Premium Plus subscribers."
- It has a headless `-p` mode that "allows easily running agents inside scripts and automations."
- It offers "full ACP support to build your own bots and agent orchestration apps."

xAI therefore advertises Grok Build for use by orchestration tools. Ostra uses it as a CLI: it runs xAI's own
binary, which signs in through xAI's own flow, and never uses xAI's OAuth client.

What is still open:

- The consumer terms were updated on 2026-09-11 and could not be fetched on 2026-09-26. The December 2024
  version forbade automated access that "sends more request messages to the servers running the Service than
  a human can reasonably produce" in the same time with a web browser. Keep `limits.max_parallel_executions`
  low on a subscription.
- API use (an xAI API key, billed per token) falls under xAI's enterprise terms. Reports say Grok Build
  accepts an API key for headless work, but xAI's announcement does not document it. Check the current
  Grok Build docs before you rely on it.
- xAI has approved subscription sign-in for some named tools (OpenCode, Hermes Agent, OpenClaw, Kilo, Warp)
  and has not published a rule for arbitrary local apps. Ostra does not need that approval, because it never
  touches the subscription sign-in, but xAI has not confirmed this reading.

### Credentials

Ostra runs each Grok execution with its own `GROK_HOME`. That directory links the entries of your real
`GROK_HOME` (`auth.json` included), so Grok reads its own sign-in (`plan_grok` in
`crates/ostra-exec-harness/src/launch.rs`). Ostra rewrites only `config.toml` and `trusted_folders.toml` in
the per-execution copy. It never opens `auth.json` and only checks that it exists to show the login status.

## Sources

- [Introducing Grok Build](https://x.ai/news/grok-build-cli)
- [SpaceXAI consumer terms of service](https://x.ai/legal/terms-of-service)
- [SpaceXAI enterprise terms of service](https://x.ai/legal/terms-of-service-enterprise)
- [xAI consumer terms, December 2024 version](https://x.ai/legal/terms-of-service/previous-2024-12-20)
- Reporting on Grok Build plans and third-party sign-in: [OpenClaw xAI docs](https://docs.openclaw.ai/providers/xai),
  [Hermes Agent guide](https://hermes-agent.nousresearch.com/docs/guides/xai-grok-oauth),
  [Yoetz issue #405](https://github.com/TheGaySupreme123/yoetz/issues/405)

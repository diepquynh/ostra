# Provider usage

This guide covers how Ostra reaches each model provider, which credentials it accepts, and the provider terms
that each path follows. It is written for people who run Ostra and for people who change its code.

Last checked against the providers' published terms on 2026-09-26. Providers change their terms without
notice, so recheck the sources listed on each provider page before a release. This guide is not legal advice.
When a use case is not covered here, ask the provider.

## Our policy

Ostra never uses a person's consumer subscription through a proxy, a copied token, or a sign-in flow of its
own. Every model call is billed to the person who runs it, through one of two paths that the provider
documents:

1. **Native executor.** Ostra calls the provider's public API with the person's own API key.
2. **Harness executor.** Ostra starts the provider's own CLI, unmodified, and the CLI signs in through the
   provider's own flow.

Ostra never does any of the following:

- Offer "Sign in with Claude", "Sign in with ChatGPT", or any other provider account login inside Ostra.
- Read, copy, parse, refresh, or send a harness CLI's stored credentials, for example
  `~/.claude/.credentials.json`, `~/.codex/auth.json`, `~/.grok/auth.json`, or the files under `~/.gemini/`.
- Ship, document, or recommend a proxy or gateway that turns a subscription into an API endpoint. This
  includes a proxy on your own machine that serves only your own subscription.
- Call a provider's private or app backend (the endpoints behind claude.ai, chatgpt.com, grok.com, or
  Google's consumer apps). Ostra calls only the documented API endpoints.
- Hold a shared key, pay for users' usage, or pass one person's credentials to another person.
- Modify a harness binary or remove any of its sign-in methods.

We hold this line because Anthropic forbids routing requests through Free, Pro, or Max credentials, Google
forbids third-party access to Antigravity, and every provider forbids sharing account credentials. A single
rule for all providers is easier to follow and to review than four separate ones.

## The two paths

| Path | Who calls the model | Credential | Billed as | Terms that apply |
| --- | --- | --- | --- | --- |
| Native (`native`) | Ostra's own agent loop | Your API key | Per token, to your API account | The provider's API or commercial terms |
| Harness (`harness:<cli>`) | The provider's own CLI | Whatever you signed that CLI in with | Your API account or your plan's usage | The CLI's terms, plus your plan's terms if you signed in with a plan |

Routing decides which path each agent takes (`[routing.executor]` in `workspace.toml`, HANDOVER 7.2).
When an agent has no route, it runs on the native path.

## What each provider allows

| Provider | Native with API key | Harness with API key | Harness with a consumer plan | Details |
| --- | --- | --- | --- | --- |
| Anthropic | Allowed | Allowed (Claude Code) | Allowed in the unmodified Claude Code binary, for ordinary individual use | [anthropic.md](anthropic.md) |
| OpenAI | Allowed | Allowed (Codex), recommended for automation | Publicly endorsed by OpenAI staff, not written into the terms | [openai.md](openai.md) |
| xAI | No native provider in Ostra | Check Grok Build's current docs | Grok Build is sold to SuperGrok and X Premium+ subscribers and advertises orchestration use | [xai.md](xai.md) |
| Google | No native provider in Ostra | Not confirmed | Not recommended: the Antigravity terms forbid use with products Google does not provide | [google.md](google.md) |

## Choosing a setup

1. **For shared, public, or unattended use, use API keys on the native path.** The terms for API keys are
   written for software like Ostra, and usage is billed per token to the key owner.
2. **For a harness, sign the CLI in with an API key when you can.** OpenAI names API keys as the default for
   programmatic Codex use, and an API key keeps harness usage under the same terms as the native path.
3. **A Claude Code or Codex harness signed in with your own plan is acceptable for your own use on your own
   machine.** Keep `limits.max_parallel_executions` low (the default is 3), because Anthropic sizes Pro and
   Max limits for ordinary individual use, and Ostra's fan-out stages can start several executions at once.
4. **Do not route agents to `harness:agy` on a Google consumer account** until Google confirms in writing that
   driving the Antigravity CLI from another tool is allowed. See [google.md](google.md).
5. **Use one person's plan for that person only.** Do not run Ostra for a team on one person's CLI login,
   because every provider forbids making an account available to anyone else.

A gateway in front of the API is fine when it holds API keys. See [gateways.md](gateways.md).

## What Ostra does in code

These facts can be checked in the source. Keep them true when you change the code; see
[contributing.md](contributing.md).

- **Native keys.** Keys come from the environment, then from credentials saved in the browser (kept in the
  registry database), then from the OS keychain (`lookup_key` in `crates/ostra-providers/src/lib.rs`). They
  never reach the browser. Anthropic calls go to `/v1/messages` and OpenAI calls to `/v1/responses`.
- **Harness environment.** Before a CLI starts, Ostra removes every provider and bridge variable from its
  environment except the ones that CLI reads to sign in (`sign_in_env` in
  `crates/ostra-exec-harness/src/launch.rs`). Claude Code keeps `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`,
  and `ANTHROPIC_BASE_URL`. Codex keeps `OPENAI_API_KEY` and `OPENAI_BASE_URL`. Grok Build and Antigravity
  keep none of Ostra's provider variables.
- **Sign-in.** Ostra runs the CLI's own login command in the Terminal view (`claude auth login`,
  `codex login`, `grok login`, or `agy`), and the sign-in completes in the provider's flow. Ostra does not see
  the result.
- **Login status.** Ostra asks the CLI (`claude auth status`, `codex login status`) or checks that the CLI's
  credential file exists (Grok Build, Antigravity). It never opens the file (`harness_status` in
  `crates/ostra-exec-harness/src/setup.rs`).
- **Expired sign-in.** When a CLI shows a sign-in screen during a run, Ostra ends the execution and reports
  that the harness needs a sign-in (`AUTH_MARKERS` in `crates/ostra-exec-harness/src/executor.rs`). It does
  not answer the sign-in screen.
- **Installers.** The setup screen runs each vendor's official installer, unchanged (`install_script`).
- **Harness launch.** Ostra passes command-line flags and writes per-execution config files (hooks, an MCP
  server entry, a system prompt). It does not patch or wrap the binary.
- **Spend.** `limits.max_parallel_executions` and `limits.session_budget_usd` in `workspace.toml` cap every
  path, and YOLO never answers a budget gate.

## Pages

- [anthropic.md](anthropic.md): Claude API and Claude Code
- [openai.md](openai.md): OpenAI API and Codex
- [xai.md](xai.md): Grok Build
- [google.md](google.md): Antigravity
- [gateways.md](gateways.md): base URLs, bearer tokens, and gateways
- [contributing.md](contributing.md): rules for code changes that touch providers or harnesses

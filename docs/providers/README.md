# Provider usage

This guide tells how Ostra connects to each model provider. It also gives the credentials that Ostra accepts,
and the provider terms that apply to each path. It is for people who run Ostra and for people who change its
code.

We last compared this guide with the published terms of the providers on 2026-09-26. Providers change their
terms without notice. Before a release, check the sources again that each provider page lists. This guide is
not legal advice. If this guide does not cover a use case, ask the provider.

## Our policy

Ostra never uses the consumer subscription of a person through a proxy, a copied token, or a sign-in flow of
its own. Each model call goes on the bill of the person who runs it. The call uses one of two paths that the
provider documents:

1. **Native executor.** Ostra calls the public API of the provider with the API key of the person.
2. **Harness executor.** Ostra starts the CLI of the provider, unmodified. The CLI signs in through the sign-in
   flow of the provider.

Ostra never does these things:

- Offer "Sign in with Claude", "Sign in with ChatGPT", or a different provider account login inside Ostra.
- Read, copy, parse, refresh, or send the stored credentials of a harness CLI. Examples of these credentials
  are `~/.claude/.credentials.json`, `~/.codex/auth.json`, `~/.grok/auth.json`, and the files under
  `~/.gemini/`.
- Ship, document, or recommend a proxy or gateway that changes a subscription into an API endpoint. This rule
  includes a proxy on your own machine that serves only your own subscription.
- Call a private or app backend of a provider. These are the endpoints behind claude.ai, chatgpt.com,
  grok.com, and the consumer apps of Google. Ostra calls only the documented API endpoints.
- Hold a shared key, pay for the usage of users, or give the credentials of one person to a different person.
- Change a harness binary or remove one of its sign-in methods.

We keep this policy for three reasons:

- Anthropic forbids the routing of requests through Free, Pro, or Max credentials.
- Google forbids third-party access to Antigravity.
- Each provider forbids the sharing of account credentials.

One rule for all providers is easier to follow and to review than four different rules.

## The two paths

| Path | Who calls the model | Credential | Billed as | Terms that apply |
| --- | --- | --- | --- | --- |
| Native (`native`) | The agent loop of Ostra | Your API key | Per token, to your API account | The API terms or commercial terms of the provider |
| Harness (`harness:<cli>`) | The CLI of the provider | The credential that you used to sign in to that CLI | Your API account or the usage of your plan | The terms of the CLI, and the terms of your plan if you signed in with a plan |

Routing sets the path of each agent (`[routing.executor]` in `workspace.toml`, HANDOVER 7.2). If an agent has
no route, it runs on the native path.

## What each provider allows

| Provider | Native with API key | Harness with API key | Harness with a consumer plan | Details |
| --- | --- | --- | --- | --- |
| Anthropic | Allowed | Allowed (Claude Code) | Allowed in the unmodified Claude Code binary, for ordinary individual use | [anthropic.md](anthropic.md) |
| OpenAI | Allowed | Allowed (Codex), recommended for automation | OpenAI staff endorse it publicly, but the terms do not state it | [openai.md](openai.md) |
| xAI | Ostra has no native provider | Read the current docs of Grok Build | xAI sells Grok Build to SuperGrok and X Premium+ subscribers and advertises it for orchestration | [xai.md](xai.md) |
| Google | Ostra has no native provider | Not confirmed | Not recommended. The Antigravity terms forbid use with products that Google does not provide | [google.md](google.md) |

## Choosing a setup

1. **For shared, public, or unattended use, use API keys on the native path.** The terms for API keys are for
   software such as Ostra. The provider bills the usage per token to the owner of the key.
2. **For a harness, sign in to the CLI with an API key when you can.** OpenAI names API keys as the default for
   programmatic Codex use. With an API key, the same terms apply to harness usage and to the native path.
3. **You can use a Claude Code or Codex harness with your own plan for your own use on your own machine.**
   Keep `limits.max_parallel_executions` low. The default is 3. Anthropic sets the Pro and Max limits for
   ordinary individual use, and the fan-out stages of Ostra can start several executions at the same time.
4. **Do not route agents to `harness:agy` on a Google consumer account.** Wait until Google confirms in writing
   that a different tool can control the Antigravity CLI. See [google.md](google.md).
5. **Use the plan of one person for that person only.** Do not run Ostra for a team on the CLI login of one
   person. Each provider forbids access to an account by a different person.

You can use a gateway in front of the API if the gateway holds API keys. See [gateways.md](gateways.md).

## What Ostra does in code

You can find these facts in the source. Keep them true when you change the code. See
[contributing.md](contributing.md).

- **Native keys.** Ostra gets keys from three sources, in this order (`lookup_key` in
  `crates/ostra-providers/src/lib.rs`):
  1. The environment.
  2. The credentials saved in the browser, which Ostra keeps in the registry database.
  3. The OS keychain.

  The keys never go to the browser. Anthropic calls go to `/v1/messages`, and OpenAI calls go to
  `/v1/responses`.
- **Harness environment.** Before a CLI starts, Ostra removes each provider and bridge variable from its
  environment (`sign_in_env` in `crates/ostra-exec-harness/src/launch.rs`). Ostra keeps only the variables that
  the CLI reads to sign in. Claude Code keeps `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, and
  `ANTHROPIC_BASE_URL`. Codex keeps `OPENAI_API_KEY` and `OPENAI_BASE_URL`. Grok Build and Antigravity keep
  none of the provider variables of Ostra.
- **Sign-in.** Ostra runs the login command of the CLI in the Terminal view: `claude auth login`,
  `codex login`, `grok login`, or `agy`. The sign-in completes in the flow of the provider. Ostra does not see
  the result.
- **Login status.** For Claude Code and Codex, Ostra asks the CLI (`claude auth status`, `codex login status`).
  For Grok Build and Antigravity, Ostra makes sure that the credential file of the CLI exists. It never opens
  the file (`harness_status` in `crates/ostra-exec-harness/src/setup.rs`).
- **Expired sign-in.** If a CLI shows a sign-in screen during a run, Ostra ends the execution. Then Ostra
  reports that the harness needs a sign-in (`AUTH_MARKERS` in `crates/ostra-exec-harness/src/executor.rs`).
  Ostra does not answer the sign-in screen.
- **Installers.** The setup screen runs the official installer of each vendor, unchanged (`install_script`).
- **Harness launch.** Ostra passes command-line flags and writes config files for each execution: hooks, an
  MCP server entry, and a system prompt. It does not patch or wrap the binary.
- **Spend.** `limits.max_parallel_executions` and `limits.session_budget_usd` in `workspace.toml` set a limit
  on each path. YOLO never answers a budget gate.

## Pages

- [anthropic.md](anthropic.md): Claude API and Claude Code
- [openai.md](openai.md): OpenAI API and Codex
- [xai.md](xai.md): Grok Build
- [google.md](google.md): Antigravity
- [gateways.md](gateways.md): base URLs, bearer tokens, and gateways
- [contributing.md](contributing.md): rules for code changes that touch providers or harnesses

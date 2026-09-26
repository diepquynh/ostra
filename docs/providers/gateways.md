# Gateways, base URLs, and bearer tokens

Ostra accepts a base URL and a bearer token so that you can send native traffic through an API gateway:

| Provider | Base URL | Credential |
| --- | --- | --- |
| Anthropic | `ANTHROPIC_BASE_URL`, `base_url`, or a URL saved in the browser | `ANTHROPIC_API_KEY` (sent as `x-api-key`) or `ANTHROPIC_AUTH_TOKEN` (sent as `Authorization: Bearer`) |
| OpenAI | `OPENAI_BASE_URL`, `base_url`, or a URL saved in the browser | `OPENAI_API_KEY` |

For the base URL, `base_url` in `config.toml` wins, then the environment variable, then the saved URL. For
the key, the environment wins, then the saved key, then the keychain.

## Allowed

Use a gateway when it forwards to the provider's public API with API keys, and the usage is billed to you or
your organization. Examples:

- A self-hosted LLM gateway that holds your Anthropic or OpenAI API keys and adds logging, budgets, or
  routing.
- Your company's gateway in front of its own API accounts or cloud inference accounts.
- A local recording or mock server for tests.

Anthropic's page on credentials confirms the first two: it "does not restrict how customers provision and
manage their own API keys or third-party inference provider credentials," as long as usage "is billed to the
key owner" and "is not resold or intermediated."

## Not allowed

Do not point Ostra at a gateway or proxy whose upstream credential is a consumer plan: Claude Free, Pro, or
Max, a ChatGPT sign-in, SuperGrok or X Premium+, or a Google AI plan. This holds even when the proxy runs on
your own machine and serves only you, for these reasons:

- Anthropic forbids routing requests "through Free, Pro, or Max plan credentials" and forbids developers to
  "collect, store, or intermediate Claude.ai credentials or session tokens."
- Its Consumer Terms forbid access "through automated or non-human means" except "via an Anthropic API Key"
  or where Anthropic explicitly permits it.
- Google's Antigravity terms forbid "using third party software, tools, or services to access the Service."

Ostra's docs, examples, tests, and issue answers never describe such a setup.

## Before you point Ostra at a gateway

1. Confirm that the gateway's upstream credential is an API key or a cloud inference credential, not a plan
   sign-in.
2. Confirm that the usage is billed to you or your organization.
3. Keep the gateway private to you or your organization.
4. Remember that the Claude Code harness inherits `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN`, and Codex
   inherits `OPENAI_BASE_URL`. When they are set, harness traffic goes through the gateway as well.

For development without spending anything, the test suite uses `ScriptedProvider` and needs no key.

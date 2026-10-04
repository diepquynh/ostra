# Gateways, base URLs, and bearer tokens

Ostra accepts a base URL and a bearer token, so you can send native traffic through an API gateway:

| Provider | Base URL | Credential |
| --- | --- | --- |
| Anthropic | `ANTHROPIC_BASE_URL`, `base_url`, or a URL saved in the browser | `ANTHROPIC_API_KEY` (sent as `x-api-key`) or `ANTHROPIC_AUTH_TOKEN` (sent as `Authorization: Bearer`) |
| OpenAI | `OPENAI_BASE_URL`, `base_url`, or a URL saved in the browser | `OPENAI_API_KEY` |

For the base URL, Ostra uses `base_url` in `config.toml` first, then the environment variable, then the saved
URL. For the key, Ostra uses the environment variable first, then the saved key, then the keychain.

## Allowed

Use a gateway when it forwards requests to the public API of the provider with API keys, and when the provider
bills the usage to you or your organization. Examples:

- A self-hosted LLM gateway that holds your Anthropic or OpenAI API keys and adds logging, budgets, or
  routing.
- The gateway of your company in front of its own API accounts or cloud inference accounts.
- A local recording server or mock server for tests.

The Anthropic page on credentials confirms the first two examples. The page says that Anthropic "does not
restrict how customers provision and manage their own API keys or third-party inference provider credentials,"
if usage "is billed to the key owner" and "is not resold or intermediated."

## Not allowed

Do not point Ostra at a gateway or proxy whose upstream credential is a consumer plan. The consumer plans are
Claude Free, Pro, or Max, a ChatGPT sign-in, SuperGrok or X Premium+, and a Google AI plan. This rule also
applies when the proxy runs on your own machine and serves only you. The reasons are:

- Anthropic does not let developers route requests "through Free, Pro, or Max plan credentials." It also
  forbids developers to "collect, store, or intermediate Claude.ai credentials or session tokens."
- The Anthropic Consumer Terms forbid access "through automated or non-human means." The exceptions are access
  "via an Anthropic API Key" and access that Anthropic explicitly permits.
- The Google Antigravity terms forbid "using third party software, tools, or services to access the Service."

The docs, examples, tests, and issue answers of Ostra never describe such a setup.

## Before you point Ostra at a gateway

1. Make sure that the upstream credential of the gateway is an API key or a cloud inference credential. A plan
   sign-in is not allowed.
2. Make sure that the provider bills the usage to you or your organization.
3. Keep the gateway private to you or your organization.
4. Check the harness variables. The Claude Code harness inherits `ANTHROPIC_BASE_URL` and
   `ANTHROPIC_AUTH_TOKEN`, and Codex inherits `OPENAI_BASE_URL`. When these variables are set, harness traffic
   also goes through the gateway.

For development at no cost, the test suite uses `ScriptedProvider`, and it needs no key.

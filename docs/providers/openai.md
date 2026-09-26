# OpenAI

Last checked: 2026-09-26. Recheck the sources at the end of this page before a release.

## Native executor: the OpenAI API

Use a Platform API key. Ostra calls the Responses API (`/v1/responses`).

```toml
# ~/.config/ostra/config.toml
[providers.openai]
api_key_env = "OPENAI_API_KEY"
# base_url_env = "OPENAI_BASE_URL"   # only for a gateway that holds API keys, see gateways.md
```

API use falls under OpenAI's services agreement for the API and its usage policies, not the consumer Terms of
Use. The Codex help article describes this split: the ChatGPT Terms of Use apply when you sign in with a
ChatGPT account, and "the corresponding online services agreement for OpenAI API" applies otherwise. Ostra
sends requests and trains no model on the output, so the rule against using output to develop competing
models does not apply.

## Harness executor: Codex

Route an agent to `harness:codex` and sign Codex in on the same machine.

### With an API key (recommended)

OpenAI's [Codex authentication page](https://learn.chatgpt.com/docs/auth) says:

- "Use API key authentication for programmatic Codex CLI workflows, such as CI/CD jobs."
- "API keys are still the recommended default for automation."
- "When you sign in with an API key, Codex uses standard API pricing instead of included ChatGPT plan
  credits."

Ostra drives Codex from a program, so sign Codex in with an API key when you can. The same page describes the
API key sign-in. Codex also inherits `OPENAI_API_KEY` and `OPENAI_BASE_URL` from Ostra's environment.

### With a ChatGPT plan

OpenAI staff, including Sam Altman, have said publicly that ChatGPT plans can be used in third-party
harnesses, and OpenAI ships a "Sign in with ChatGPT" flow for external apps. No clause in the terms says this,
though, so treat it as tolerated rather than guaranteed. Ostra goes further than those harnesses: it runs
OpenAI's own Codex CLI and never handles the ChatGPT sign-in.

The limits that do appear in OpenAI's terms:

- "You may not share your account credentials or make your account available to anyone else." Use your plan
  for your own work only.
- OpenAI's terms forbid automated or programmatic extraction of data or output. The general Terms of Use
  could not be fetched on 2026-09-26; the related
  [Teacher Access Terms](https://openai.com/policies/education-terms/) list "Automatically or programmatically
  extract data or Output." We read Codex driven by its own signed-in user as ordinary Codex use, not
  extraction, but OpenAI has not said so in writing.
- The authentication page says: "Don't expose Codex execution in untrusted or public environments." Keep
  Ostra's server bound to your machine or behind its sign-in when a Codex harness is signed in.

### Credentials

Codex stores its sign-in "in a plaintext file at `~/.codex/auth.json` or in your OS-specific credential
store," and OpenAI says to "treat `~/.codex/auth.json` like a password." Ostra never opens that file. It
checks sign-in with `codex login status` and starts sign-in with `codex login`.

## Sources

- [Codex authentication](https://learn.chatgpt.com/docs/auth)
- [Using Codex with your ChatGPT plan](https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan)
- [OpenAI Terms of Use](https://openai.com/policies/row-terms-of-use/)
- [OpenAI Service terms](https://openai.com/policies/service-terms/)
- [Codex for Open Source Program Terms](https://learn.chatgpt.com/docs/codex-for-oss-terms)
- [GitHub discussion on Codex forks and "Sign in with ChatGPT"](https://github.com/openai/codex/discussions/8338)
- Reporting on OpenAI's public statements: [Manifest](https://manifest.build/blog/chatgpt-plus-tokens-third-party-harnesses/),
  [MindStudio](https://www.mindstudio.ai/blog/anthropic-restricts-third-party-agents-openai-opens-codex-comparison)

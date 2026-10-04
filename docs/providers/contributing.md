# Rules for code that touches providers or harnesses

These rules keep the policy in [README.md](README.md) true when the code changes. A change that breaks one of
these rules does not merge.

## Model access

1. **Add no provider account sign-in.** Ostra must not contain an OAuth client, a device-code flow, or a login
   page for Anthropic, OpenAI, xAI, or Google model access. The native path takes API keys. The harness path
   lets the CLI do the sign-in. OAuth for workspace MCP servers (HANDOVER 10.6) is a different feature, and it
   stays allowed.
2. **Call only documented public API endpoints.** A native provider connects to the provider's published API
   (for example `/v1/messages` or `/v1/responses`). It can also connect to a gateway that the user configures.
   Never send requests to the backend of claude.ai, chatgpt.com, grok.com, or Google's consumer apps.
3. **A new native provider takes an API key.** Add it to `Providers::rebuild` and give it `api_key_env`. Before
   it ships, document its terms on a new page in this folder.

## Harness credentials

4. **Never read, copy, parse, refresh, or send the stored credentials of a CLI.** To check the login status, use
   the status command of the CLI, or check that a file exists (`harness_status` in `setup.rs`). A per-execution
   home can link the real files of the CLI so that the CLI can read them. `plan_grok` does this. But Ostra's
   code must not open these files.
5. **Keep `sign_in_env` minimal.** A harness keeps only the provider variables that its own CLI reads to sign
   in. Ostra removes each other name from `credential_env_names` before the launch. Put a new credential variable
   in `DEFAULT_CREDENTIAL_ENV` or get it from `[providers.*]`, so that the removal covers it.
6. **Never answer a sign-in screen.** If a CLI asks for sign-in during a run, end the execution with
   `End::Auth`. Then the person signs in from the Terminal view.

## Harness binaries

7. **Run the binary of the vendor as the vendor publishes it.** Configure it with flags, environment variables,
   and per-execution config files. Do not patch, wrap, or replace the binary. Do not pass a value that removes
   or disables one of its sign-in methods. Anthropic sets these two conditions for the use of Claude Code in a
   different product.
8. **Install only with the official installer of the vendor** (`install_script`), and do not change it.

## Spend

9. **Every execution goes through the slot limiter and the session budget.** Keep the default
   `max_parallel_executions` low. Anthropic sets plan limits for usual individual use. Also, xAI's older terms
   limit the request rate to the rate that a person can send.

## Docs and examples

10. **Examples use API keys.** No doc, test, fixture, script, or issue answer shows a proxy that serves a
    consumer plan as an API.
11. **Update this folder when the terms of a provider change.** Change the "Last checked" date on each page that
    you check again. Quote the new wording, and update the table in [README.md](README.md).

## Review checklist

- [ ] No new code reads a file under `~/.claude`, `~/.codex`, `~/.grok`, or `~/.gemini`. The only exceptions:
      code that checks that a file exists, and code that writes Ostra's own plugin files.
- [ ] No new OAuth or login flow for model access.
- [ ] No new endpoint outside the documented API of a provider.
- [ ] Harness launch changes are flags or config files only.
- [ ] Each new fan-out goes through the slot limiter.
- [ ] The docs under `docs/providers/` still agree with the code that they cite.

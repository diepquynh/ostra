# Rules for code that touches providers or harnesses

These rules keep the policy in [README.md](README.md) true as the code changes. A change that breaks one of
them does not merge.

## Model access

1. **Add no provider account sign-in.** Ostra must not contain an OAuth client, device-code flow, or login page
   for Anthropic, OpenAI, xAI, or Google model access. The native path takes API keys; the harness path leaves
   sign-in to the CLI. OAuth for workspace MCP servers (HANDOVER 10.6) is a separate feature and stays
   allowed.
2. **Call only documented public API endpoints.** A native provider talks to the provider's published API (for
   example `/v1/messages` or `/v1/responses`) or to a gateway the user configures. Never target the backend
   of claude.ai, chatgpt.com, grok.com, or Google's consumer apps.
3. **A new native provider takes an API key.** Add it to `Providers::rebuild`, give it `api_key_env`, and
   document its terms on a new page in this folder before it ships.

## Harness credentials

4. **Never read, copy, parse, refresh, or send a CLI's stored credentials.** Check login status with the CLI's
   own status command or by checking that a file exists (`harness_status` in `setup.rs`). A per-execution home
   may link the CLI's real files so the CLI can read them, as `plan_grok` does, but Ostra's code must not open
   them.
5. **Keep `sign_in_env` minimal.** A harness keeps only the provider variables its own CLI reads to sign in.
   Every other name from `credential_env_names` is removed before launch. A new credential variable goes into
   `DEFAULT_CREDENTIAL_ENV` or comes from `[providers.*]`, so the removal covers it.
6. **Never answer a sign-in screen.** When a CLI asks for sign-in during a run, end the execution with
   `End::Auth` and let the person sign in from the Terminal view.

## Harness binaries

7. **Run the vendor's binary as published.** Configure it with flags, environment variables, and
   per-execution config files. Do not patch, wrap, or replace the binary, and do not pass anything that
   removes or disables one of its sign-in methods. Anthropic makes both conditions for running Claude Code in
   another product.
8. **Install only with the vendor's official installer** (`install_script`), unchanged.

## Spend

9. **Every execution goes through the slot limiter and the session budget.** Keep the default
   `max_parallel_executions` low, because Anthropic sizes plan limits for ordinary individual use and xAI's
   older terms cap request rates at what a person could send.

## Docs and examples

10. **Examples use API keys.** No doc, test, fixture, script, or issue answer shows a proxy that serves a
    consumer plan as an API.
11. **Update this folder when a provider's terms change.** Change the "Last checked" date on each page you
    recheck, quote the new wording, and update the table in [README.md](README.md).

## Review checklist

- [ ] No new code reads a file under `~/.claude`, `~/.codex`, `~/.grok`, or `~/.gemini` other than to check
      that it exists or to write Ostra's own plugin files.
- [ ] No new OAuth or login flow for model access.
- [ ] No new endpoint outside a provider's documented API.
- [ ] Harness launch changes are flags or config files only.
- [ ] Any new fan-out goes through the slot limiter.
- [ ] Docs under `docs/providers/` still match the code they cite.

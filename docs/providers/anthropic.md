# Anthropic

Last checked: 2026-09-26. Recheck the sources at the end of this page before a release.

## Native executor: the Claude API

Use an API key from the Claude Console.

```toml
# ~/.config/ostra/config.toml
[providers.anthropic]
api_key_env = "ANTHROPIC_API_KEY"
# keychain_service = "ostra"   # optional: read the key from the OS keychain, account "anthropic"
```

You can also save the key from the browser, where it is stored in the registry database. The environment
variable wins over a saved key.

API keys fall under the [Commercial Terms of Service](https://www.anthropic.com/legal/commercial-terms)
(effective June 17, 2025) and the [Usage Policy](https://www.anthropic.com/legal/aup). The clauses that
matter for Ostra:

- **A.1** allows using the Services "to power products and services Customer makes available to its own
  customers and end users." Ostra is such a product, and each person who runs it with their own key is the
  customer.
- **D.4** forbids accessing the Services "to build a competing product or service, including to train
  competing AI models," reselling the Services "except as expressly approved by Anthropic," and reverse
  engineering them. Ostra sends requests to Claude, trains no model, and resells nothing.
- **D.5**: "Customer is responsible for all activity under its account." The key owner answers for every
  execution Ostra runs with that key, so set a session budget.

Do not put a Claude subscription token in `ANTHROPIC_AUTH_TOKEN`, and do not point `ANTHROPIC_BASE_URL` at a
proxy that holds one. The Claude Code
[legal and compliance page](https://code.claude.com/docs/en/legal-and-compliance) says: "Anthropic does not
permit third-party developers to offer Claude.ai login into their own applications, or to route requests
through Free, Pro, or Max plan credentials on behalf of their users. Moreover, developers may not collect,
store, or intermediate Claude.ai credentials or session tokens." `ANTHROPIC_AUTH_TOKEN` exists for gateways
that hold API keys; see [gateways.md](gateways.md).

## Harness executor: Claude Code

Route an agent to `harness:claude` and sign Claude Code in on the same machine, with either an API key or
your own Claude plan.

The same Anthropic page allows a Claude plan in the unmodified Claude Code binary. The rule on third-party
credentials "does not ... prevent an end user from signing in to the unmodified Claude Code binary with their
own Claude subscription, including where a platform hosts Claude Code." Products that run Claude Code must
meet these conditions. Each row says how Ostra meets it:

| Condition from Anthropic | How Ostra meets it |
| --- | --- |
| "The Claude Code binary must not be modified." | Ostra runs the `claude` on `PATH` (or `[harness.claude] command`), installed by Anthropic's installer. It passes flags only: `--settings`, `--setting-sources user`, `--mcp-config`, `--strict-mcp-config`, `--append-system-prompt-file`, `--tools`, `--model`, `--effort`, and so on. |
| Customers "may not remove, disable, or restrict any authentication method built into it." | Ostra passes no flag or setting that changes sign-in. The person signs in with `claude auth login`. |
| Customers "may not pay for, resell, or intermediate Claude usage on their end users' behalf." | Each person signs in with their own account on their own machine. Ostra holds no key and relays nothing. |
| Names and logos: you can say, in plain text, that your product "runs Claude Code," but not use Anthropic's names or logos in your product name or logo, or suggest endorsement. | Ostra's name and logo contain neither. Docs and UI name Claude Code in plain text only. |

Anthropic also says: "Advertised usage limits for Pro and Max plans assume ordinary, individual usage of
Claude Code and the Agent SDK." On a plan, keep `limits.max_parallel_executions` at 3 or lower, and do not run
Ostra for other people on your login. The [Consumer Terms](https://www.anthropic.com/legal/consumer-terms)
(effective October 8, 2025) forbid it: "You may not share your Account login information, Anthropic API key,
or Account credentials with anyone else."

Claude Code inherits `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, and `ANTHROPIC_BASE_URL` from Ostra's
environment. When they are set, harness traffic follows them as well, so set them only to an API key or to a
gateway that holds API keys. To run Claude Code on your plan while native agents use an API key, save the key
from the browser (or in the keychain) instead of exporting it.

## Enforcement so far

- 2026-01-09: server-side checks started rejecting subscription OAuth tokens outside Claude Code.
- 2026-02-19: the rule was written into the Claude Code legal and compliance page.
- 2026-04-04: Claude plans stopped covering usage through third-party harnesses such as OpenClaw and
  OpenCode. Those tools now need an API key or pay-as-you-go extra usage.

Anthropic "may do so without prior notice." Ostra is unaffected because its native path uses API keys and
its harness path runs Claude Code itself.

## Sources

- [Claude Code: Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)
- [Anthropic Commercial Terms of Service](https://www.anthropic.com/legal/commercial-terms)
- [Anthropic Consumer Terms of Service](https://www.anthropic.com/legal/consumer-terms)
- [Anthropic Usage Policy](https://www.anthropic.com/legal/aup)
- [Anthropic Trademark Guidelines](https://www.anthropic.com/legal/trademark-guidelines)
- Reporting on the enforcement dates: [GIGAZINE](https://gigazine.net/gsc_news/en/20260220-anthropic-third-party-block/),
  [WinBuzzer](https://winbuzzer.com/2026/02/19/anthropic-bans-claude-subscription-oauth-in-third-party-apps-xcxwbn/),
  [MindStudio](https://www.mindstudio.ai/blog/anthropic-openclaw-ban-oauth-authentication)

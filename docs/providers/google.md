# Google

Last checked: 2026-09-26. Recheck the sources at the end of this page before a release.

## Native executor

Ostra has no native Gemini provider. The native registry builds only `anthropic` and `openai`.

## Harness executor: Antigravity

**Do not route agents to `harness:agy` while the CLI is signed in with a Google consumer account** (a Google
AI plan or a free Google account) until Google confirms in writing that another tool may drive the
Antigravity CLI.

The [Antigravity additional terms](https://antigravity.google/terms), section 6, list these as prohibited:

- "using the Service in connection with products not provided by us."
- "Using third party software, tools, or services to access the Service (e.g. using OpenClaw with Antigravity
  OAuth)."

And: "Such actions may be grounds for suspension or termination of your Antigravity and/or Gemini CLI
accounts."

Ostra runs Google's own `agy` binary, which signs in through Google's flow, and never reads Antigravity's
OAuth token. That is a narrower use than the OpenClaw example, which reused the OAuth token directly. The first
clause is broad, though, and can cover a tool that drives `agy`. Google has acted on these terms: since
February 2026 it has suspended accounts, including paid AI Ultra accounts, that used Antigravity or Gemini CLI
sign-ins in third-party tools. Suspensions also cut off Gemini CLI and Gemini Code Assist on the same account.
Reports from early September 2026 say enforcement is still going on.

Ostra keeps the `agy` harness for three cases: a Google account whose terms allow it, Google's written
confirmation, or an API-key sign-in if the CLI supports one. The terms page does not say whether `agy` accepts
a Gemini API key or Vertex AI credentials; check Google's current Antigravity CLI docs.

### What Ostra installs

The Antigravity harness needs one global integration: a plugin in `~/.gemini/config/plugins/ostra/` with one
agent file per Ostra agent and a hooks file. Every hook is inert unless `OSTRA_EXECUTION` is set, so the
plugin does nothing outside an Ostra execution (`crates/ostra-exec-harness/src/setup.rs`). Ostra checks that
`~/.gemini/antigravity-cli/antigravity-oauth-token` or `~/.gemini/oauth_creds.json` exists to show the login
status, and never opens either file. The launch keeps none of Ostra's provider variables.

## Sources

- [Google Antigravity additional terms of service](https://antigravity.google/terms)
- [Gemini CLI discussion #20632: Addressing Antigravity bans](https://github.com/google-gemini/gemini-cli/discussions/20632)
- [Google AI Developers Forum: 403 ToS bans on Antigravity and Gemini CLI](https://discuss.ai.google.dev/t/urgent-mass-403-tos-bans-on-gemini-api-antigravity-for-open-source-cli-users-paid-tier/124508)
- [oh-my-pi issue #12487: Gemini auth safe by default](https://github.com/can1357/oh-my-pi/issues/12487)
- Reporting on enforcement: [Enterprise DNA, 2026-09-03](https://enterprisedna.co/resources/ai-pulse/ai-pulse-2026-09-03-google-suspends-paying-antigravity-subscribers-for-using-thi/),
  [MLQ News](https://mlq.ai/news/google-enforces-tos-bans-on-paid-antigravity-subscribers-using-openclaw-tool/)

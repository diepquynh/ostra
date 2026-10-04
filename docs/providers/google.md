# Google

Last checked: 2026-09-26. Recheck the sources at the end of this page before a release.

## Native executor

Ostra has no native Gemini provider. The native registry builds only `anthropic` and `openai`.

## Harness executor: Antigravity

**Do not route agents to `harness:agy` when the CLI uses a sign-in with a Google consumer account.** A Google
consumer account is a Google AI plan or a free Google account. This rule applies until Google confirms in
writing that another tool can control the Antigravity CLI.

Section 6 of the [Antigravity additional terms](https://antigravity.google/terms) lists these actions as
prohibited:

- "using the Service in connection with products not provided by us."
- "Using third party software, tools, or services to access the Service (e.g. using OpenClaw with Antigravity
  OAuth)."

Section 6 also says: "Such actions may be grounds for suspension or termination of your Antigravity and/or
Gemini CLI accounts."

Ostra runs the `agy` binary of Google, which signs in through the Google flow. Ostra never reads the OAuth
token of Antigravity. This use is narrower than the OpenClaw example, which reused the OAuth token directly. But
the first clause is broad, and it can cover a tool that controls `agy`.

Google acts on these terms. From February 2026, Google suspended accounts that used Antigravity or Gemini CLI
sign-ins in third-party tools. These accounts included paid AI Ultra accounts. A suspension also stops Gemini
CLI and Gemini Code Assist on the same account. Reports from early September 2026 say that the enforcement
continues.

Ostra keeps the `agy` harness for three cases:

- A Google account whose terms allow this use.
- A written confirmation from Google.
- An API-key sign-in, if the CLI supports one.

The terms page does not say whether `agy` accepts a Gemini API key or Vertex AI credentials. Check the current
Antigravity CLI docs of Google.

### What Ostra installs

The Antigravity harness needs one global integration. This integration is a plugin in
`~/.gemini/config/plugins/ostra/`, with one agent file for each Ostra agent and a hooks file. Each hook does
nothing unless `OSTRA_EXECUTION` is set. So the plugin does nothing outside an Ostra execution
(`crates/ostra-exec-harness/src/setup.rs`). To show the login status, Ostra checks that
`~/.gemini/antigravity-cli/antigravity-oauth-token` or `~/.gemini/oauth_creds.json` exists. Ostra never opens
either file. The launch keeps none of the provider variables of Ostra.

## Sources

- [Google Antigravity additional terms of service](https://antigravity.google/terms)
- [Gemini CLI discussion #20632: Addressing Antigravity bans](https://github.com/google-gemini/gemini-cli/discussions/20632)
- [Google AI Developers Forum: 403 ToS bans on Antigravity and Gemini CLI](https://discuss.ai.google.dev/t/urgent-mass-403-tos-bans-on-gemini-api-antigravity-for-open-source-cli-users-paid-tier/124508)
- [oh-my-pi issue #12487: Gemini auth safe by default](https://github.com/can1357/oh-my-pi/issues/12487)
- Reporting on enforcement: [Enterprise DNA, 2026-09-03](https://enterprisedna.co/resources/ai-pulse/ai-pulse-2026-09-03-google-suspends-paying-antigravity-subscribers-for-using-thi/),
  [MLQ News](https://mlq.ai/news/google-enforces-tos-bans-on-paid-antigravity-subscribers-using-openclaw-tool/)

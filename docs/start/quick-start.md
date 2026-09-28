# Quick start

This page takes you from a clean machine to a first finished session in about fifteen minutes, most of which
is the first Rust build. It skips the explanations. [Install](install.md) covers each step in depth, and
[Your first session](first-session.md) walks through what you see on screen.

## 1. Install the prerequisites

Ostra runs on Linux and macOS. You need:

- [rustup](https://rustup.rs). The repository pins its toolchain in `rust-toolchain.toml`, and rustup installs
  that version on the first build.
- Node.js 24 with npm, which builds the browser console that the binary embeds.
- A C compiler (`build-essential` on Debian and Ubuntu, the Xcode command line tools on macOS) and `git`.
- On Linux only, bubblewrap: `sudo apt install bubblewrap` or `sudo dnf install bubblewrap`. macOS uses its
  built-in Seatbelt sandbox, so it needs nothing extra.

Install bubblewrap before the first run, because the sandbox defaults to `required` and Ostra refuses to start
any agent command it cannot sandbox.

## 2. Give Ostra a model

The shortest path is an API key in the shell that starts Ostra:

```bash
export ANTHROPIC_API_KEY=sk-ant-...   # or OPENAI_API_KEY
```

You can also skip this and paste a key into the setup screen later, or use a harness CLI (Claude Code, Codex,
Grok Build, Antigravity) that is already signed in. [Provider usage](../providers/README.md) lists every path.

## 3. Build and start

```bash
git clone https://github.com/diepquynh/ostra && cd ostra
./run.sh
```

`run.sh` installs the npm packages, builds the console, builds the release binary, and starts it. Ostra prints
a sign-in URL and opens it in your browser:

```
Ostra is running on 127.0.0.1:7878. Open this URL to sign in:

  http://ostra-4c1e9a.localhost:7878/#token=3f2a...
```

The host is a private `*.localhost` name for this install, because a cookie set on `127.0.0.1` would be sent
to every other program listening on this machine. The link works once and for 15 minutes. If the tab does not open, or you close it, run
`./target/release/ostra url` in another terminal for a fresh one.

## 4. Create a workspace

The setup screen checks the machine for keys and harness CLIs, then asks for:

1. A workspace name and a folder. The folder holds the workspace's settings and every session's files.
2. The projects to work on: existing folders, or a git URL to clone.
3. Default permissions and model routing. The defaults are fine for a first run.

## 5. Initialize a project

Open the project from the left dock and press **Initialize**. Ostra scouts the code, proposes a set of
project skills, and stops at a skill approval gate. Accept the defaults. When it finishes, the project has an
`.ostra/INVENTORY.md` and a `.ostra/project.toml`, and the pipeline can target it.

Initialization runs several executions in parallel, so it is the most expensive step of a first run. The
workspace's session budget (25 USD by default) applies to it.

## 6. Run a task

Open the workspace screen, type a small, concrete request into **New task**, and start it. A good first
request touches one file:

```
Add a --version flag to the CLI that prints the version from Cargo.toml.
```

A request this small is likely classified QUICK CHANGE: one implementer pass, no spec or plan, and the
changed files staged in git. The session board shows which category the Classify judge picked and why.

For a larger request the board researches the code and then builds the change in reviewed phases. A change
the research shows needs settled requirements takes the full track first, and you answer the gates it raises:
open questions, spec approval, and plan approval when the change is not low-stakes. When the last phase passes
review, you try the change and send feedback until you accept it. Then the closing gate asks whether to write
tests and docs, and the completion report summarizes what changed. The
changed files are staged in git for you to review and commit.

## 7. Stop the server

Press Ctrl-C in the terminal. Before you start it again, end any session you do not want resumed:

```bash
./target/release/ostra stop s_01a0cdab436570b0b522e10ce263a904
```

Ostra resumes interrupted work on the next start, which re-runs every execution that was running when it
stopped. [Troubleshooting](troubleshooting.md) covers this and the other common surprises.

## Next

- [The pipeline](../internals/pipeline.md): what each stage does and why it exists.
- [Architecture overview](../architecture/overview.md): how the server, engine, and executors fit together.
- [Threat model](../security/threat-model.md): what Ostra defends against when an agent runs on your machine.

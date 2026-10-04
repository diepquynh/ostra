# Quick start

This page takes you from a clean machine to a first finished session in about fifteen minutes. The first Rust
build uses most of that time. This page does not give the explanations. [Install](install.md) covers each step
in depth. [Your first session](first-session.md) describes what you see on the screen.

## 1. Install the prerequisites

Ostra runs on Linux and macOS. You need these items:

- [rustup](https://rustup.rs). The repository pins its toolchain in `rust-toolchain.toml`. Rustup installs
  that version on the first build.
- Node.js 24 with npm. Npm builds the browser console that the binary embeds.
- A C compiler and `git`. On Debian and Ubuntu, install `build-essential`. On macOS, install the Xcode command
  line tools.
- On Linux only, bubblewrap: `sudo apt install bubblewrap` or `sudo dnf install bubblewrap`. macOS uses its
  built-in Seatbelt sandbox, so it needs no other package.

Install bubblewrap before the first run. The sandbox defaults to `required`, and Ostra refuses to start an
agent command that it cannot sandbox.

## 2. Give Ostra a model

The shortest path is an API key in the shell that starts Ostra:

```bash
export ANTHROPIC_API_KEY=sk-ant-...   # or OPENAI_API_KEY
```

You can also skip this step and paste a key into the setup screen later. Or you can use a harness CLI that is
already signed in: Claude Code, Codex, Grok Build, or Antigravity. [Provider usage](../providers/README.md)
lists every path.

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

The host is a private `*.localhost` name for this install. The reason: a browser sends a cookie set on
`127.0.0.1` to every other program that listens on this machine. The link works one time and for 15 minutes.
If the tab does not open, or if you close it, run `./target/release/ostra url` in another terminal to get a
new link.

## 4. Create a workspace

The setup screen looks for keys and harness CLIs on the machine. Then it asks for these items:

1. A workspace name and a folder. The folder holds the settings of the workspace and the files of each session.
2. The projects to work on: existing folders, or a git URL to clone.
3. Default permissions and model routing. The defaults are correct for a first run.

## 5. Initialize a project

Open the project from the left dock and push **Initialize**. Ostra scouts the code, proposes a set of project
skills, and stops at a skill approval gate. Accept the defaults. When initialization finishes, the project has
an `.ostra/INVENTORY.md` and a `.ostra/project.toml`. Then the pipeline can target the project.

Initialization runs several executions in parallel, so it is the most expensive step of a first run. The
session budget of the workspace applies to it. The default budget is 25 USD.

## 6. Run a task

Open the workspace screen. Type a small, specific request into **New task**, and start it. A good first
request changes one file:

```
Add a --version flag to the CLI that prints the version from Cargo.toml.
```

The Classify judge probably puts a request this small in the QUICK CHANGE category. That category has one
implementer pass and no spec or plan, and it stages the changed files in git. The session board shows which
category the Classify judge picked and the reason.

For a larger request, the board researches the code. Then it builds the change in reviewed phases. If the
research shows that the change needs settled requirements, the change takes the full track first. Then you
answer the gates that it opens:

- Open questions.
- Spec approval.
- Plan approval, when the change is not low-stakes.

When the last phase passes review, you try the change and send feedback until you accept it. Then the closing
gate asks whether to write tests and docs. The completion report summarizes what changed. Ostra stages the
changed files in git for you to review and commit.

## 7. Stop the server

Push Ctrl-C in the terminal. Before you start the server again, end each session that you do not want Ostra to
resume:

```bash
./target/release/ostra stop s_01a0cdab436570b0b522e10ce263a904
```

On the next start, Ostra resumes interrupted work. It runs again each execution that was running when the
server stopped. [Troubleshooting](troubleshooting.md) covers this behavior and the other common surprises.

## Next

- [The pipeline](../internals/pipeline.md): what each stage does and why it exists.
- [Architecture overview](../architecture/overview.md): how the server, the engine, and the executors work together.
- [Threat model](../security/threat-model.md): what Ostra defends against when an agent runs on your machine.

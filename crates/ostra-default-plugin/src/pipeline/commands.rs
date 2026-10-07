//! Rule D8 and Step 2: the format and stage commands, which the build and closing stages both run.

use ostra_core::event::{CommandPurpose, SessionEvent};
use ostra_core::ids::SessionId;
use ostra_engine::runner::{EngineError, StepHost, run_host, run_shell};

/// The output of a format step skipped because its command waits for approval.
pub const FORMAT_NOT_APPROVED: &str = "The format command in project.toml changed outside Ostra, so format was skipped. Approve it in the project's settings to run it next time.";

/// Rule D8 and Step 2: run the project's format command, or stage the files a loop changed, and
/// record what ran.
pub(crate) async fn perform_command(
    host: &StepHost,
    session: &SessionId,
    purpose: CommandPurpose,
    project: &str,
    command: Option<String>,
    files: Vec<String>,
) -> Result<(), EngineError> {
    let root = host
        .snapshot(session)?
        .project_path(project)
        .unwrap_or_default();
    let started = |command: &str| {
        host.append(
            session,
            SessionEvent::CommandStarted {
                purpose,
                project: project.into(),
                command: command.into(),
            },
        )
        .map(|_| ())
    };
    let (cmd_text, exit, tail) = match purpose {
        CommandPurpose::Format => match command {
            None => (
                String::new(),
                None,
                "No format command in project.toml, so format was skipped.".to_string(),
            ),
            // Rule A1: a format command runs only once the user approved it.
            Some(cmd) if !host.services().command_approved(&root, &cmd) => {
                (cmd, None, FORMAT_NOT_APPROVED.to_string())
            }
            Some(cmd) => {
                started(&cmd)?;
                // The project's own program, so it runs under the agent sandbox.
                let (code, out) = match ostra_sandbox::host_command(
                    "bash",
                    &["-c".into(), cmd.clone()],
                    &root,
                    &[&root],
                    &host.services().workspace().sandbox(),
                ) {
                    Ok(hc) => run_host(&root, &hc, 600).await,
                    Err(e) => (None, e),
                };
                (cmd, code, out)
            }
        },
        CommandPurpose::Stage => {
            if files.is_empty() {
                (String::new(), Some(0), "No files to stage.".to_string())
            } else {
                // Rule WD2: a multi-project phase stages each project's files in its own repo.
                let st = host.snapshot(session)?;
                let groups =
                    crate::stages::build::hooks::files_by_project(&st, project, files.iter());
                let (mut texts, mut code, mut outs) = (vec![], Some(0), vec![]);
                for (key, files) in groups {
                    let Some(root) = st.project_path(&key) else {
                        continue;
                    };
                    // Staging keeps each review focused on the unstaged diff (Step 2).
                    for p in ostra_sandbox::repair_git_dirs(&ostra_sandbox::git_repos(&[&root])) {
                        tracing::warn!("removed a planted {} before staging", p.display());
                    }
                    let mut args: Vec<String> = ostra_core::git::AUTOMATIC
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    args.extend(ostra_core::git::filter_overrides(&root).await);
                    args.extend([
                        "-C".into(),
                        root.display().to_string(),
                        "add".into(),
                        "-A".into(),
                        "--".into(),
                    ]);
                    args.extend(files);
                    let text = format!("git {}", args.join(" "));
                    started(&text)?;
                    let (c, out) = run_shell(&root, "git", &args, 60).await;
                    if c != Some(0) {
                        code = c;
                    }
                    texts.push(text);
                    outs.push(out);
                }
                (texts.join("\n"), code, outs.join("\n"))
            }
        }
        CommandPurpose::Autofix => (String::new(), Some(0), String::new()),
    };
    host.append(
        session,
        SessionEvent::CommandRan {
            purpose,
            project: project.into(),
            command: cmd_text,
            exit_code: exit,
            output_tail: tail,
        },
    )?;
    Ok(())
}

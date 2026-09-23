//! Build and test signal extraction plus the per-execution failure streak. Ports Ultracode's
//! `hooks/lib/build-signal.js`, `hooks/build-streak.js`, and `hooks/build-streak-gate.js`.

use ostra_core::policy::ToolOutcome;
use regex::Regex;
use std::sync::LazyLock;

/// Recall fires one failure before the warning, so a known fix arrives at attempt 2.
pub const RECALL_THRESHOLD: u32 = 2;
pub const WARN_THRESHOLD: u32 = 3;
pub const DENY_THRESHOLD: u32 = 5;
const MAX_HISTORY: usize = 12;

// Used only when the project has no configured commands (build-signal.js FALLBACK_BUILD).
static FALLBACK_BUILD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(^|[\s|;&(])(\./mvnw|mvn|\./gradlew|gradle|tsc|cargo|dotnet|make|pytest|go\s+(build|test|vet)|npm\s+run\s+\S+|npm\s+(test|run-script)|pnpm\s+(test|run|build)|yarn\s+(test|run|build)|uv\s+run\s+(pytest|mypy|ruff)|python\s+-m\s+(pytest|mypy|unittest))\b",
    )
    .unwrap()
});

static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{[A-Za-z_][A-Za-z0-9_]*\}").unwrap());

/// A configured command becomes a matcher: `{PLACEHOLDER}` tokens are wildcards and whitespace
/// is flexible, so `./mvnw test -pl {MODULE}` matches the concrete invocation.
pub fn command_matcher(configured: &str) -> Option<Regex> {
    let parts: Vec<String> = PLACEHOLDER
        .split(configured)
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| {
            p.split_whitespace()
                .map(regex::escape)
                .collect::<Vec<_>>()
                .join(r"\s+")
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    Regex::new(&parts.join(r"[\s\S]*?")).ok()
}

/// Is this shell command a build or test invocation for this project?
pub fn is_build_command(command: &str, configured: &[String]) -> bool {
    let text = command.trim();
    if text.is_empty() {
        return false;
    }
    let configured: Vec<&str> = configured
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if configured
        .iter()
        .any(|c| command_matcher(c).is_some_and(|m| m.is_match(text)))
    {
        return true;
    }
    // The driver head of a configured command (`./mvnw`, `uv run pytest`) still counts: agents
    // vary flags around the same build tool.
    for c in &configured {
        let head = c.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        if head.len() > 2 && text.contains(&head) {
            return true;
        }
    }
    configured.is_empty() && FALLBACK_BUILD.is_match(text)
}

static FAILURE_MARKERS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)BUILD FAILURE",
        r"(?i)BUILD FAILED",
        r"(?i)Compilation failure",
        r"(?i)COMPILATION ERROR",
        r"\bFAILURES!+",
        r"(?i)Tests? run:.*?(?:Failures|Errors):\s*[1-9]",
        r"(?i)\b\d+ failed\b",
        r"(?m)^\s*FAIL\b",
        r"npm ERR!",
        r"error TS\d+",
        r"(?m)^error(\[E\d+\])?:",
        r"(?i)\bmypy\b.*\berror\b",
        r"Traceback \(most recent call last\)",
        r"=+ FAILURES =+",
        r"(?i)spotless.*violations",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

static EXIT_CODE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?m)^\s*Exit code (\d+)",
        r"(?i)\bexited with code (\d+)",
        r"(?i)\bexit status (\d+)",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

static INTERRUPTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(timed out|interrupted|canceled|cancelled)\b").unwrap());

pub fn exit_code_from(text: &str) -> Option<i32> {
    EXIT_CODE_PATTERNS.iter().find_map(|p| {
        p.captures(text)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse().ok())
    })
}

/// Did the command fail? `None` means no evidence either way (interrupted, timed out, or a
/// harness that reports neither an exit code nor output).
pub fn failed_from(outcome: &ToolOutcome) -> Option<bool> {
    if let Some(code) = outcome.exit_code {
        return Some(code != 0);
    }
    let text = &outcome.output;
    if let Some(code) = exit_code_from(text) {
        return Some(code != 0);
    }
    if outcome.is_error && INTERRUPTED.is_match(text) {
        return None;
    }
    if outcome.is_error {
        return Some(true);
    }
    if !outcome.result_known && text.trim().is_empty() {
        return None;
    }
    if FAILURE_MARKERS.iter().any(|m| m.is_match(text)) {
        return Some(true);
    }
    Some(false)
}

static DIAGNOSTIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)cannot find symbol|does not exist|incompatible type|argument lists differ|is not assignable|no suitable method|has private access|unreported exception|duplicate class|cannot be applied|Unresolved reference|Cannot resolve|error TS\d+|error\[E\d+\]|AssertionError|NullPointerException|IllegalStateException|IllegalArgumentException|UnsatisfiedDependency|NoSuchBean|BeanCreation|ImportError|ModuleNotFoundError|AttributeError|TypeError|NameError|violations|expected .{0,40} but",
    )
    .unwrap()
});

static NORM_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*\[?(ERROR|WARN|E|FAIL)\]?\s*").unwrap());
static NORM_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/[\w.@/-]+").unwrap());
static NORM_POS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\d+[,:]\d+\]").unwrap());
static NORM_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":\d+(?::\d+)?").unwrap());
static NORM_VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d[\d.]{2,}\b").unwrap());
static NORM_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

pub fn normalize_diagnostic(line: &str) -> String {
    let s = NORM_PREFIX.replace(line, "");
    let s = NORM_PATH.replace_all(&s, "<path>");
    let s = NORM_POS.replace_all(&s, "");
    let s = NORM_LINE.replace_all(&s, "");
    let s = NORM_VERSION.replace_all(&s, "<v>");
    let s = NORM_SPACE.replace_all(&s, " ");
    s.trim().chars().take(160).collect()
}

/// A stable one-line fingerprint of why a build failed, or `None` for an unknown cause.
pub fn diagnostic_signature(text: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim();
        if t.len() < 12 || t.len() > 400 || !DIAGNOSTIC.is_match(t) {
            continue;
        }
        let n = normalize_diagnostic(t);
        if n.len() >= 8 {
            return Some(n);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    pub signature: String,
    pub streak: u32,
}

/// One execution's consecutive build failures and unrecorded recoveries.
#[derive(Debug, Clone, Default)]
pub struct Streak {
    pub consecutive: u32,
    pub last_signature: Option<String>,
    history: Vec<Option<String>>,
    /// Verified failure-then-recovery transitions still waiting for a lesson.
    pub pending: Vec<Recovery>,
}

/// What the streak wants said after a build outcome.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreakEvent {
    pub recall: Option<String>,
    pub note: Option<String>,
}

impl Streak {
    pub fn record(&mut self, failed: bool, output: &str) -> StreakEvent {
        let mut ev = StreakEvent::default();
        if !failed {
            // A pass after a real streak is a verified recovery: the moment a lesson is worth recording.
            if self.consecutive >= WARN_THRESHOLD
                && let Some(sig) = self.last_signature.clone()
            {
                let streak = self.consecutive;
                self.pending.push(Recovery {
                    signature: sig.clone(),
                    streak,
                });
                if self.pending.len() > MAX_HISTORY {
                    self.pending.remove(0);
                }
                ev.note = Some(format!(
                    "Record what fixed this now, with the Memory tool: that passed after {streak} consecutive failures on \"{sig}\". \
                     Use area = the affected module and a one-line lesson naming the diagnostic and the fix (the root cause \
                     and the correct pattern, not a narration of your debugging), because this diagnostic recurs across \
                     sessions and a recorded lesson stops the next run from re-deriving it. Your report and your submit call \
                     are refused until the lesson is recorded."
                ));
            }
            self.consecutive = 0;
            self.last_signature = None;
            return ev;
        }
        let sig = diagnostic_signature(output);
        self.consecutive += 1;
        if sig.is_some() {
            self.last_signature = sig.clone();
        }
        self.history.push(sig.clone());
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
        let count = self.consecutive;
        if count < RECALL_THRESHOLD {
            return ev;
        }
        ev.recall = sig.clone();
        if (WARN_THRESHOLD..DENY_THRESHOLD).contains(&count) {
            let repeated = sig.as_ref().is_some_and(|s| {
                self.history
                    .iter()
                    .filter(|h| h.as_ref() == Some(s))
                    .count()
                    > 1
            });
            let remaining = DENY_THRESHOLD - count;
            let mut w = format!(
                "State what you now believe the root cause is, and what specifically will change, before the next attempt: \
                 that is {count} consecutive failing build or test commands in this run"
            );
            if repeated && let Some(s) = &sig {
                w.push_str(&format!(", and the same diagnostic is repeating (\"{s}\")"));
            }
            w.push_str(&format!(
                ". Do not retry a variation of the same edit. After {remaining} more consecutive failure{} further build and \
                 test commands are refused, and you must call your submit tool with status `stuck` (the STUCK: hand-back).",
                if remaining == 1 { "" } else { "s" }
            ));
            ev.note = Some(w);
        } else if count >= DENY_THRESHOLD {
            ev.note = Some(format!(
                "Call your submit tool with status `stuck` now: that is {count} consecutive failing build or test commands, \
                 so further build and test commands are refused."
            ));
        }
        ev
    }

    pub fn blocked(&self) -> bool {
        self.consecutive >= DENY_THRESHOLD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAIL: &str = "Exit code 1\n[ERROR] /r/src/main/java/Foo.java:[11,2] cannot find symbol\n[ERROR] Failed to execute goal maven-compiler-plugin:3.14.1:compile (default-compile) on project core: Compilation failure";

    fn out(text: &str) -> ToolOutcome {
        ToolOutcome {
            output: text.into(),
            is_error: false,
            exit_code: None,
            result_known: true,
        }
    }

    #[test]
    fn build_commands_from_profile() {
        let cmds = vec![
            "./mvnw -q -T1C compile".to_string(),
            "./mvnw test -Ptest".to_string(),
            "./mvnw test -Ptest -pl {MODULE} -am -Dtest={TEST}".to_string(),
        ];
        assert!(is_build_command("./mvnw -q -T1C compile", &cmds));
        assert!(is_build_command(
            "./mvnw test -Ptest -pl core -am -Dtest=FooTest",
            &cmds
        ));
        assert!(!is_build_command("ls -la src", &cmds));
        assert!(!is_build_command("cat src/main/java/Foo.java", &cmds));
        assert!(is_build_command("cargo test -p x", &[]));
        assert!(!is_build_command("cargo test -p x", &["npm test".into()]));
    }

    #[test]
    fn failure_detection() {
        assert_eq!(failed_from(&out(FAIL)), Some(true));
        assert_eq!(failed_from(&out("BUILD SUCCESS")), Some(false));
        assert_eq!(
            failed_from(&out("The command exited with code 1.")),
            Some(true)
        );
        assert_eq!(
            failed_from(&ToolOutcome {
                exit_code: Some(0),
                ..out("BUILD FAILURE")
            }),
            Some(false)
        );
        assert_eq!(
            failed_from(&ToolOutcome {
                is_error: true,
                ..out("Command timed out after 120s")
            }),
            None
        );
        assert_eq!(
            failed_from(&ToolOutcome {
                result_known: false,
                ..out("")
            }),
            None
        );
    }

    #[test]
    fn signature_is_normalized() {
        let s = diagnostic_signature(FAIL).unwrap();
        assert!(s.contains("cannot find symbol"));
        assert!(s.contains("<path>"));
        assert!(!s.contains("11,2"));
    }

    #[test]
    fn streak_thresholds() {
        let mut s = Streak::default();
        assert_eq!(s.record(true, FAIL), StreakEvent::default());
        let e2 = s.record(true, FAIL);
        assert!(e2.recall.is_some() && e2.note.is_none());
        let e3 = s.record(true, FAIL);
        let w = e3.note.unwrap();
        assert!(w.contains("3 consecutive failing build or test commands"));
        assert!(w.contains("same diagnostic is repeating"));
        assert!(w.contains("stuck"));
        s.record(true, FAIL);
        assert!(!s.blocked());
        s.record(true, FAIL);
        assert!(s.blocked());
        let rec = s.record(false, "BUILD SUCCESS");
        assert!(
            rec.note
                .unwrap()
                .contains("passed after 5 consecutive failures")
        );
        assert_eq!(s.pending.len(), 1);
        assert_eq!(s.pending[0].streak, 5);
        assert!(!s.blocked());
        s.record(false, "BUILD SUCCESS");
        assert_eq!(s.pending.len(), 1, "a plain pass records no recovery");
    }
}

//! The shells agent commands run in. On Windows each is found by absolute path, because the
//! first `bash` on `PATH` is often `C:\Windows\System32\bash.exe`, the WSL launcher, which runs
//! the command in a Linux VM with a different view of the files.

use std::path::PathBuf;
#[cfg(windows)]
use std::sync::OnceLock;

/// The program the Bash tool runs: `bash` from `PATH` on Unix, Git for Windows' `bin\bash.exe`
/// on Windows.
pub fn bash() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        static FOUND: OnceLock<Result<PathBuf, String>> = OnceLock::new();
        FOUND.get_or_init(find_git_bash).clone()
    }
    #[cfg(not(windows))]
    Ok(PathBuf::from("bash"))
}

/// What to do when Git Bash is missing, for the tool error and the setup check.
#[cfg(windows)]
pub const NO_GIT_BASH: &str = "Install Git for Windows (https://git-scm.com/download/win) on the machine that runs Ostra, then restart Ostra, because the Bash tool runs commands in Git Bash and Ostra does not use the WSL `bash.exe`.";

/// The root of the Git for Windows install, from `git --exec-path`
/// (`<root>/mingw64/libexec/git-core`), else the default install dirs.
#[cfg(windows)]
pub fn git_root() -> Option<PathBuf> {
    let from_git = std::process::Command::new("git")
        .arg("--exec-path")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let exec = PathBuf::from(String::from_utf8_lossy(&o.stdout).trim());
            exec.ancestors()
                .find(|a| a.join("bin").join("bash.exe").is_file())
                .map(PathBuf::from)
        });
    from_git.or_else(|| {
        [
            std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("Git")),
            std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Programs").join("Git")),
        ]
        .into_iter()
        .flatten()
        .find(|r| r.join("bin").join("bash.exe").is_file())
    })
}

#[cfg(windows)]
fn find_git_bash() -> Result<PathBuf, String> {
    git_root()
        .map(|r| r.join("bin").join("bash.exe"))
        .ok_or_else(|| format!("Git Bash was not found. {NO_GIT_BASH}"))
}

/// Windows PowerShell 5.1, which every Windows 10 and 11 install has, under the system dir.
#[cfg(windows)]
pub fn powershell() -> PathBuf {
    system_root()
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe")
}

/// `cmd.exe` under the system dir, not the first one on `PATH`.
#[cfg(windows)]
pub fn cmd() -> PathBuf {
    system_root().join("System32").join("cmd.exe")
}

#[cfg(windows)]
fn system_root() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn git_bash_is_not_the_wsl_launcher() {
        let Ok(bash) = super::bash() else {
            eprintln!("Git for Windows is not installed; skipping");
            return;
        };
        assert!(bash.is_absolute() && bash.is_file(), "{}", bash.display());
        let lower = bash.to_string_lossy().to_ascii_lowercase();
        assert!(!lower.contains(r"\system32\"), "{lower}");
        assert!(super::powershell().is_file() && super::cmd().is_file());
    }
}

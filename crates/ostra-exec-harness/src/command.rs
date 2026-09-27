//! How a harness command name becomes a program Windows can start.

use std::path::{Path, PathBuf};

/// A program to start, and the arguments that go before the caller's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub program: PathBuf,
    pub prefix: Vec<String>,
}

/// Resolve `command` the way a shell would, for spawns that search `PATH` themselves. Unix starts
/// the name as given.
#[cfg(not(windows))]
pub fn resolve(command: &str) -> Result<Resolved, String> {
    Ok(Resolved {
        program: PathBuf::from(command),
        prefix: vec![],
    })
}

/// npm installs a CLI as three files: an extensionless `sh` script for Git Bash, a `.cmd`, and a
/// `.ps1`. CreateProcessW cannot start the first, and `cmd.exe` cannot pass the multi-line
/// arguments a harness receives, so an npm shim runs its script with `node` directly.
#[cfg(windows)]
pub fn resolve(command: &str) -> Result<Resolved, String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let found = find(
        command,
        &std::env::split_paths(&path).collect::<Vec<_>>(),
        &pathext,
    )
    .ok_or_else(|| {
        format!(
            "Install `{command}` or name its full path, because it is not on this machine's PATH."
        )
    })?;
    from_file(&found, &path)
}

/// The first file named `command` with a `PATHEXT` extension, in `PATH` order. A name that already
/// ends in one is also taken as it is.
#[cfg_attr(not(windows), allow(dead_code))]
fn find(command: &str, dirs: &[PathBuf], pathext: &str) -> Option<PathBuf> {
    let exts: Vec<String> = pathext
        .split(';')
        .filter(|e| e.starts_with('.'))
        .map(|e| e.to_ascii_lowercase())
        .collect();
    let has_ext = Path::new(command)
        .extension()
        .is_some_and(|e| exts.contains(&format!(".{}", e.to_string_lossy().to_ascii_lowercase())));
    let candidates = |base: PathBuf| {
        let mut out = vec![];
        if has_ext {
            out.push(base.clone());
        }
        for e in &exts {
            let mut name = base.clone().into_os_string();
            name.push(e);
            out.push(PathBuf::from(name));
        }
        out
    };
    let bases: Vec<PathBuf> = if Path::new(command).components().count() > 1 {
        vec![PathBuf::from(command)]
    } else {
        dirs.iter().map(|d| d.join(command)).collect()
    };
    bases.into_iter().flat_map(candidates).find(|p| p.is_file())
}

#[cfg_attr(not(windows), allow(dead_code))]
fn from_file(found: &Path, path: &std::ffi::OsStr) -> Result<Resolved, String> {
    let ext = found
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if ext != "cmd" && ext != "bat" {
        return Ok(Resolved {
            program: found.to_path_buf(),
            prefix: vec![],
        });
    }
    let text = std::fs::read_to_string(found).unwrap_or_default();
    let dir = found.parent().unwrap_or(Path::new("."));
    let script = npm_shim_script(&text).ok_or_else(|| {
        format!(
            "Set the harness command to an .exe, because {} is a batch file and cmd.exe cannot pass the multi-line arguments a harness receives.",
            found.display()
        )
    })?;
    let local = dir.join("node.exe");
    let node = if local.is_file() {
        local
    } else {
        find(
            "node",
            &std::env::split_paths(path).collect::<Vec<_>>(),
            ".EXE",
        )
        .ok_or_else(|| {
            format!(
                "Install Node.js, because {} is an npm command that runs with node.",
                found.display()
            )
        })?
    };
    Ok(Resolved {
        program: node,
        prefix: vec![dir.join(script).to_string_lossy().into_owned()],
    })
}

#[cfg_attr(not(windows), allow(dead_code))]
/// The script an npm `cmd-shim` runs: the quoted `%dp0%\...` path before `%*`.
fn npm_shim_script(text: &str) -> Option<String> {
    let re = regex::Regex::new(r#""%_prog%"\s+"%dp0%\\([^"]+)"\s+%\*"#).ok()?;
    re.captures(text).map(|c| c[1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIM: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n";

    #[test]
    fn reads_the_script_of_an_npm_shim() {
        assert_eq!(
            npm_shim_script(SHIM).as_deref(),
            Some(r"node_modules\@openai\codex\bin\codex.js")
        );
        assert_eq!(npm_shim_script("@echo off\r\nfoo.exe %*\r\n"), None);
    }

    #[test]
    fn skips_the_extensionless_sh_script() {
        let tmp = tempfile::tempdir().unwrap();
        let npm = tmp.path().join("npm");
        std::fs::create_dir_all(npm.join(r"node_modules/@openai/codex/bin")).unwrap();
        std::fs::write(npm.join("codex"), "#!/bin/sh\n").unwrap();
        std::fs::write(npm.join("codex.cmd"), SHIM).unwrap();
        std::fs::write(npm.join("node.exe"), "").unwrap();
        let found = find("codex", std::slice::from_ref(&npm), ".COM;.EXE;.BAT;.CMD").unwrap();
        assert_eq!(found, npm.join("codex.cmd"));
        let r = from_file(&found, std::ffi::OsStr::new("")).unwrap();
        assert_eq!(r.program, npm.join("node.exe"));
        assert_eq!(
            r.prefix,
            vec![
                npm.join(r"node_modules\@openai\codex\bin\codex.js")
                    .to_string_lossy()
                    .into_owned()
            ]
        );
    }

    #[test]
    fn prefers_an_exe_in_an_earlier_dir_and_refuses_other_batch_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("claude.exe"), "").unwrap();
        std::fs::write(b.join("claude.cmd"), SHIM).unwrap();
        let exts = ".COM;.EXE;.BAT;.CMD";
        assert_eq!(
            find("claude", &[a.clone(), b.clone()], exts),
            Some(a.join("claude.exe"))
        );
        assert_eq!(
            find("claude", &[b.clone(), a.clone()], exts),
            Some(b.join("claude.cmd"))
        );
        std::fs::write(b.join("tool.bat"), "@echo off\r\ntool.exe %*\r\n").unwrap();
        let err = from_file(&b.join("tool.bat"), std::ffi::OsStr::new("")).unwrap_err();
        assert!(
            err.starts_with("Set the harness command to an .exe"),
            "{err}"
        );
    }
}

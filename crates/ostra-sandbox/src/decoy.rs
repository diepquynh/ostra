//! Decoy credential files (Rule P3): credential paths an agent has no reason to read, so opening
//! one from inside the sandbox is a containment signal. A Bash `cat` is seen as well as a tool
//! call, because the kernel reports it.
//!
//! - Linux: a fake file is bound read-only into the sandbox and watched with inotify from outside.
//! - macOS: Seatbelt cannot bind a file in, so a decoy is an existing file the policy refuses to
//!   read, and the refusal is read from the system log (`log stream`), which macOS shows to admin
//!   accounts only. A missing file raises no report, so only existing files become decoys there.

use crate::backend::Backend;
use crate::sys::{Os, Platform};
use ostra_core::paths;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Called once per decoy with the path the sandbox saw it at.
pub type OnOpen = Arc<dyn Fn(PathBuf) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    SshKey,
    GitCredentials,
    VaultToken,
    Generic,
}

impl Kind {
    /// The kind of a workspace's own decoy at `rel` under the home folder.
    fn for_rel(rel: &str) -> Kind {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        if rel.starts_with(".ssh/") && name.starts_with("id_") && !name.ends_with(".pub") {
            Kind::SshKey
        } else {
            Kind::Generic
        }
    }
}

/// Decoys under the home folder. Common tools do not open these on their own, apart from `ssh`,
/// which reads its keys when it authenticates, so the keys are planted only where no SSH port is
/// reachable. `~/.netrc` is left out because Python `requests` and Go module downloads read it
/// on every fetch.
pub const HOME_DECOYS: &[(&str, Kind)] = &[
    (".ssh/id_rsa", Kind::SshKey),
    (".ssh/id_ed25519", Kind::SshKey),
    (".git-credentials", Kind::GitCredentials),
    (".vault-token", Kind::VaultToken),
];

/// Decoys one workspace may add (`sandbox_decoys`), because each is a watch and a bind mount.
pub const MAX_WORKSPACE_DECOYS: usize = 32;

/// The built-in decoys followed by a workspace's own, as `(path under the home folder, kind)`.
/// A workspace adds to the built-in list and cannot remove from it.
pub fn wanted(extra: &[String]) -> Vec<(String, Kind)> {
    let mut out: Vec<(String, Kind)> = HOME_DECOYS
        .iter()
        .map(|(rel, kind)| (rel.to_string(), *kind))
        .collect();
    for e in extra {
        let Some(rel) = e.trim().strip_prefix("~/") else {
            continue;
        };
        if !out.iter().any(|(r, _)| r == rel) {
            out.push((rel.to_string(), Kind::for_rel(rel)));
        }
    }
    out
}

/// Why `entry` cannot be a workspace decoy, with the correction first.
pub fn invalid(entry: &str) -> Option<String> {
    let rel = entry.trim().strip_prefix("~/");
    let clean = rel.is_some_and(|r| {
        !r.is_empty()
            && !r.contains('\0')
            && r.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
    });
    if !clean {
        return Some(format!(
            "Write `{entry}` as a file path starting with `~/`, such as `~/.aws/credentials`, because decoys live in the home folder."
        ));
    }
    let rel = rel.unwrap_or_default();
    if paths::HOME_CREDENTIALS
        .iter()
        .any(|c| c.harness.is_some() && c.path == rel)
    {
        return Some(format!(
            "Remove `{entry}`, because it is a harness CLI's sign-in file and a decoy there would sign that CLI out in the sandbox."
        ));
    }
    None
}

pub fn dir() -> PathBuf {
    paths::data_dir().join("decoys")
}

/// Removes decoys a server of this data dir left behind. Call once at server start, while no
/// other server of the data dir runs.
pub fn remove_stale() -> usize {
    let Ok(rd) = std::fs::read_dir(dir()) else {
        return 0;
    };
    rd.flatten()
        .filter(|e| std::fs::remove_dir_all(e.path()).is_ok())
        .count()
}

fn random(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n + 16);
    while out.len() < n {
        out.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    out.truncate(n);
    out
}

fn text(n: usize, alphabet: &[u8]) -> String {
    random(n)
        .into_iter()
        .map(|b| alphabet[b as usize % alphabet.len()] as char)
        .collect()
}

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const HEX: &[u8] = b"0123456789abcdef";

/// Plausible contents that authenticate nowhere.
fn contents(kind: Kind) -> String {
    match kind {
        Kind::SshKey => {
            let body = text(70 * 5, B64);
            let lines: Vec<&str> = body
                .as_bytes()
                .chunks(70)
                .map(|c| std::str::from_utf8(c).unwrap_or_default())
                .collect();
            format!(
                "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA{}\n-----END OPENSSH PRIVATE KEY-----\n",
                lines.join("\n")
            )
        }
        Kind::GitCredentials => format!("https://git:{}@github.com\n", text(40, HEX)),
        Kind::VaultToken => format!("hvs.{}", text(24, B64)),
        Kind::Generic => format!("{}\n", text(40, HEX)),
    }
}

/// Starts every Seatbelt message that marks a decoy.
pub(crate) const TAG_PREFIX: &str = "dev.ostra.decoy.";

fn new_tag() -> String {
    format!(
        "{TAG_PREFIX}{}",
        &uuid::Uuid::new_v4().simple().to_string()[..16]
    )
}

/// One execution's decoys, watched until dropped.
#[derive(Debug)]
pub struct Decoys {
    dir: PathBuf,
    /// `(host file, path inside the sandbox)`.
    binds: Vec<(PathBuf, PathBuf)>,
    /// `(message, path)`: each refused path and the message its Seatbelt rule logs.
    tags: Vec<(String, PathBuf)>,
    /// Taken first on drop, so the watch ends before its files go.
    watch: Option<<Platform as Os>::DecoyWatch>,
}

impl Decoys {
    /// Writes a decoy for each `(path inside the sandbox, kind)` into a private dir of its own
    /// under `base` ([`dir`]) and starts watching them. `on` runs on the watcher's thread.
    /// Where the OS cannot watch opens (anywhere but Linux), `Err`.
    pub fn plant(base: &Path, at: &[(PathBuf, Kind)], on: OnOpen) -> std::io::Result<Decoys> {
        use std::io::Write;
        paths::create_private_dir_all(base)?;
        let dir = base.join(&uuid::Uuid::new_v4().simple().to_string()[..16]);
        paths::create_private_dir(&dir)?;
        let mut binds = vec![];
        for (i, (dest, kind)) in at.iter().enumerate() {
            let src = dir.join(i.to_string());
            paths::private_file_options()
                .write(true)
                .create_new(true)
                .open(&src)?
                .write_all(contents(*kind).as_bytes())?;
            binds.push((src, dest.clone()));
        }
        let watch = match Platform::watch_opens(&binds, on) {
            Ok(w) => w,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
        };
        Ok(Decoys {
            dir,
            binds,
            tags: vec![],
            watch: Some(watch),
        })
    }

    /// Watches for the policy's refusals to read each existing file in `at`, which it refuses
    /// with the message [`Decoys::tags`] names. `on` runs on the reader's thread. Where the OS
    /// reports no refusals (anywhere but macOS), `Err`.
    pub fn refuse(at: &[PathBuf], on: OnOpen) -> std::io::Result<Decoys> {
        let tags: Vec<(String, PathBuf)> = at.iter().map(|p| (new_tag(), p.clone())).collect();
        let watch = Platform::watch_refusals(&tags, on)?;
        Ok(Decoys {
            dir: PathBuf::new(),
            binds: vec![],
            tags,
            watch: Some(watch),
        })
    }

    pub fn binds(&self) -> &[(PathBuf, PathBuf)] {
        &self.binds
    }

    /// `(message, path)` for each file [`Decoys::refuse`] watches.
    pub fn tags(&self) -> &[(String, PathBuf)] {
        &self.tags
    }

    pub fn host_dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Decoys {
    fn drop(&mut self) {
        self.watch = None;
        if !self.dir.as_os_str().is_empty() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Whether decoys work on this machine: under bubblewrap always, under Seatbelt when this user
/// may read the system log, which macOS allows admin accounts only.
pub fn supported(backend: &Backend) -> bool {
    backend.enforcer().decoys_supported()
}

/// Whether this user may read the refusals Seatbelt decoys are reported by.
pub(crate) fn refusals_readable() -> bool {
    Platform::refusals_readable()
}

/// Starts reading Seatbelt's reports from the system log, so the first execution's decoys are
/// watched from its first command. Call once at server start. Does nothing where decoys do not
/// use the log.
pub fn watch_reports() {
    Platform::start_refusal_reports();
}

#[cfg(test)]
mod rules {
    use super::*;

    #[test]
    fn a_workspace_adds_decoys_and_never_removes_the_built_in_ones() {
        let w = wanted(&[
            "~/.aws/credentials".into(),
            "~/.ssh/id_rsa".into(),
            "~/.ssh/id_work".into(),
        ]);
        let rels: Vec<&str> = w.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(
            rels,
            [
                ".ssh/id_rsa",
                ".ssh/id_ed25519",
                ".git-credentials",
                ".vault-token",
                ".aws/credentials",
                ".ssh/id_work"
            ]
        );
        assert_eq!(w[4].1, Kind::Generic);
        assert_eq!(w[5].1, Kind::SshKey);
        assert_eq!(wanted(&[]).len(), HOME_DECOYS.len());
    }

    #[test]
    fn workspace_decoys_are_home_files_that_sign_no_harness_out() {
        assert_eq!(invalid("~/.aws/credentials"), None);
        assert_eq!(invalid("~/.config/app/token.json"), None);
        for bad in [
            "/etc/shadow",
            "~/",
            "~/../x",
            "~/a//b",
            "~/a/./b",
            "~/.aws/",
            "relative",
            "~/.claude/.credentials.json",
        ] {
            assert!(invalid(bad).is_some(), "{bad}");
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    fn wait_for(seen: &Mutex<Vec<PathBuf>>, n: usize) {
        let start = Instant::now();
        while seen.lock().unwrap().len() < n && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn each_decoy_reports_its_first_open_once() {
        let base = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(vec![]));
        let s = seen.clone();
        let d = Decoys::plant(
            base.path(),
            &[
                ("/home/u/.ssh/id_rsa".into(), Kind::SshKey),
                ("/home/u/.vault-token".into(), Kind::VaultToken),
            ],
            Arc::new(move |p| s.lock().unwrap().push(p)),
        )
        .unwrap();
        let (key, _) = &d.binds()[0];
        let body = std::fs::read_to_string(key).unwrap();
        assert!(body.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n"));
        std::fs::read(key).unwrap();
        // Metadata alone is not an open.
        std::fs::metadata(&d.binds()[1].0).unwrap();
        wait_for(&seen, 1);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            *seen.lock().unwrap(),
            vec![PathBuf::from("/home/u/.ssh/id_rsa")]
        );
        let dir = d.host_dir().to_path_buf();
        drop(d);
        assert!(!dir.exists());
    }
}

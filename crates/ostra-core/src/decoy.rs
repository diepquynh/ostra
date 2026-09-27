//! Decoy credential files (Rule P3): fake files at credential paths the sandbox hides. Opening
//! one from inside the sandbox is a containment signal, because an agent has no reason to read
//! them. Linux only: each file is bound read-only into the sandbox and watched with inotify from
//! outside it, so a Bash `cat` is seen as well as a tool call.

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
    if crate::paths::HOME_CREDENTIALS
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
    crate::paths::data_dir().join("decoys")
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

/// One execution's decoys, watched until dropped.
#[derive(Debug)]
pub struct Decoys {
    dir: PathBuf,
    /// `(host file, path inside the sandbox)`.
    binds: Vec<(PathBuf, PathBuf)>,
    #[cfg(target_os = "linux")]
    watch: linux::Watch,
}

impl Decoys {
    /// Writes a decoy for each `(path inside the sandbox, kind)` into a private dir of its own
    /// under `base` ([`dir`]) and starts watching them. `on` runs on the watcher's thread.
    pub fn plant(base: &Path, at: &[(PathBuf, Kind)], on: OnOpen) -> std::io::Result<Decoys> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(base)?;
        let dir = base.join(&uuid::Uuid::new_v4().simple().to_string()[..16]);
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let mut binds = vec![];
        for (i, (dest, kind)) in at.iter().enumerate() {
            let src = dir.join(i.to_string());
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&src)?
                .write_all(contents(*kind).as_bytes())?;
            binds.push((src, dest.clone()));
        }
        #[cfg(target_os = "linux")]
        {
            let watch = match linux::Watch::start(&binds, on) {
                Ok(w) => w,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(e);
                }
            };
            Ok(Decoys { dir, binds, watch })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (on, std::fs::remove_dir_all(&dir));
            Err(std::io::ErrorKind::Unsupported.into())
        }
    }

    pub fn binds(&self) -> &[(PathBuf, PathBuf)] {
        &self.binds
    }

    pub fn host_dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Decoys {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        self.watch.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::OnOpen;
    use std::collections::HashMap;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[derive(Debug)]
    pub(super) struct Watch {
        fd: Arc<OwnedFd>,
        wds: Vec<i32>,
    }

    impl Watch {
        pub(super) fn start(binds: &[(PathBuf, PathBuf)], on: OnOpen) -> std::io::Result<Watch> {
            // SAFETY: inotify_init1 takes no pointers; a negative return is an error.
            let raw = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
            if raw < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: `raw` is a fresh descriptor nothing else owns.
            let fd = Arc::new(unsafe { OwnedFd::from_raw_fd(raw) });
            let mut names = HashMap::new();
            for (src, dest) in binds {
                let c = std::ffi::CString::new(src.as_os_str().as_bytes())?;
                // SAFETY: `c` is a NUL-terminated path that outlives the call.
                let wd =
                    unsafe { libc::inotify_add_watch(fd.as_raw_fd(), c.as_ptr(), libc::IN_OPEN) };
                if wd < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                names.insert(wd, dest.clone());
            }
            let wds = names.keys().copied().collect();
            let reader = fd.clone();
            std::thread::Builder::new()
                .name("ostra-decoy-watch".into())
                .spawn(move || read(&reader, names, on))?;
            Ok(Watch { fd, wds })
        }

        /// Removes every watch. The kernel answers each with `IN_IGNORED`, which ends the reader.
        pub(super) fn stop(&self) {
            for wd in &self.wds {
                // SAFETY: plain syscall on a descriptor this struct keeps open; a watch the reader
                // already removed gives EINVAL.
                unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), *wd) };
            }
        }
    }

    fn read(fd: &OwnedFd, mut names: HashMap<i32, PathBuf>, on: OnOpen) {
        const HEAD: usize = std::mem::size_of::<libc::inotify_event>();
        let mut live = names.len();
        let mut buf = vec![0u8; 4096];
        while live > 0 {
            // SAFETY: reads into a buffer of the length passed.
            let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                if n < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                    continue;
                }
                return;
            }
            let n = n as usize;
            let mut off = 0;
            while off + HEAD <= n {
                // SAFETY: the kernel wrote a whole event header at `off`; it may be unaligned.
                let ev: libc::inotify_event =
                    unsafe { std::ptr::read_unaligned(buf.as_ptr().add(off).cast()) };
                if ev.mask & libc::IN_IGNORED != 0 {
                    live -= 1;
                } else if ev.mask & libc::IN_OPEN != 0 {
                    // One signal per decoy: later opens of the same file are not watched.
                    if let Some(path) = names.remove(&ev.wd) {
                        // SAFETY: as in `Watch::stop`.
                        unsafe { libc::inotify_rm_watch(fd.as_raw_fd(), ev.wd) };
                        on(path);
                    }
                }
                off += HEAD + ev.len as usize;
            }
        }
    }
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

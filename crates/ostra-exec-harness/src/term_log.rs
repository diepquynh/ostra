//! The raw bytes of a harness terminal, kept on disk so the Terminal tab can replay an ended run.
//! The file grows to at most twice [`TRANSCRIPT_CAP`]; past that it is rewritten with its last
//! [`TRANSCRIPT_CAP`] bytes, so writes stay appends and memory stays flat. Harness output can
//! carry secrets, so the file is readable by its owner only.

use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Bytes of terminal output a replay shows.
pub const TRANSCRIPT_CAP: usize = 2 * 1024 * 1024;

pub struct TermLog {
    path: PathBuf,
    cap: usize,
    state: Mutex<State>,
}

struct State {
    file: Option<File>,
    len: usize,
}

impl std::fmt::Debug for TermLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermLog").field("path", &self.path).finish()
    }
}

impl TermLog {
    /// Start an empty transcript at `path`, replacing any earlier one.
    pub fn create(path: PathBuf) -> std::io::Result<Self> {
        Self::with_cap(path, TRANSCRIPT_CAP)
    }

    fn with_cap(path: PathBuf, cap: usize) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let file = private(&path)?;
        Ok(TermLog {
            path,
            cap,
            state: Mutex::new(State {
                file: Some(file),
                len: 0,
            }),
        })
    }

    /// Append terminal output. A failed write stops the transcript; the run itself goes on.
    pub fn append(&self, bytes: &[u8]) {
        let mut s = self.state.lock();
        let Some(file) = s.file.as_mut() else { return };
        if file.write_all(bytes).is_err() {
            s.file = None;
            return;
        }
        s.len += bytes.len();
        if s.len > self.cap * 2 {
            s.file = self.trim().ok();
            s.len = s.file.as_ref().map_or(0, |_| self.cap);
        }
    }

    fn trim(&self) -> std::io::Result<File> {
        let bytes = std::fs::read(&self.path)?;
        let tail = &bytes[bytes.len().saturating_sub(self.cap)..];
        let tmp = self.path.with_extension("log.tmp");
        private(&tmp)?.write_all(tail)?;
        std::fs::rename(&tmp, &self.path)?;
        OpenOptions::new().append(true).open(&self.path)
    }
}

/// Create or truncate `path` as a file only its owner can read.
fn private(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // `mode` applies only on creation; an older transcript keeps its bits otherwise.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// The stored transcript at `path`, at most [`TRANSCRIPT_CAP`] bytes. `None` when there is none.
pub fn read_transcript(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let start = bytes.len().saturating_sub(TRANSCRIPT_CAP);
    Some(bytes[start..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_last_bytes_under_the_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x").join("terminal.log");
        let log = TermLog::with_cap(path.clone(), 10).unwrap();
        assert_eq!(read_transcript(&path), None);
        log.append(b"hello ");
        assert_eq!(read_transcript(&path).unwrap(), b"hello ");
        for i in 0..10u8 {
            log.append(&[b'0' + i; 3]);
        }
        let stored = std::fs::read(&path).unwrap();
        assert!(stored.len() <= 20, "{}", stored.len());
        assert!(stored.ends_with(b"888999"));
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600, "a trimmed transcript stays private");
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        let log = TermLog::with_cap(path.clone(), 10).unwrap();
        log.append(b"new");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"new",
            "a new run replaces the old transcript"
        );
    }
}

//! Decoy opens, reported by inotify on the host files bound into the sandbox.

use crate::decoy::OnOpen;
use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct Watch {
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
            let wd = unsafe { libc::inotify_add_watch(fd.as_raw_fd(), c.as_ptr(), libc::IN_OPEN) };
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
}

impl Drop for Watch {
    /// Removes every watch. The kernel answers each with `IN_IGNORED`, which ends the reader.
    fn drop(&mut self) {
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
            if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
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

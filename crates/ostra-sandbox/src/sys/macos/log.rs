//! Seatbelt's decoy refusals, read from the system log (`log stream`), which macOS shows to
//! admin accounts only.

use crate::decoy::{OnOpen, TAG_PREFIX};
use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Only the kernel's Sandbox extension, because any process may log text of its own.
const PREDICATE: &str = "processID == 0 AND senderImagePath ENDSWITH \"/Sandbox\" AND eventMessage CONTAINS \"dev.ostra.decoy.\"";

type Registry = Mutex<HashMap<String, (PathBuf, OnOpen)>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Tags reported to their callback until dropped.
#[derive(Debug)]
pub(crate) struct Registration(Vec<String>);

impl Drop for Registration {
    fn drop(&mut self) {
        let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
        for tag in &self.0 {
            r.remove(tag);
        }
    }
}

pub(super) fn register(tags: &[(String, PathBuf)], on: OnOpen) -> std::io::Result<Registration> {
    if !reader() {
        return Err(std::io::Error::other(
            "the system log cannot be read, so Seatbelt's refusals would go unseen",
        ));
    }
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    for (tag, path) in tags {
        r.insert(tag.clone(), (path.clone(), on.clone()));
    }
    Ok(Registration(tags.iter().map(|(t, _)| t.clone()).collect()))
}

/// Whether this process's user is in the `admin` group, which `log stream` requires.
pub(super) fn admin() -> bool {
    const ADMIN_GID: libc::gid_t = 80;
    // SAFETY: the first call asks for the count; the second fills a buffer of that size.
    unsafe {
        let n = libc::getgroups(0, std::ptr::null_mut());
        if n <= 0 {
            return false;
        }
        let mut groups: Vec<libc::gid_t> = vec![0; n as usize];
        let n = libc::getgroups(n, groups.as_mut_ptr());
        n > 0 && groups[..n as usize].contains(&ADMIN_GID)
    }
}

/// Starts the log reader once per process and waits until `log stream` is attached, so no
/// report from a command that starts right after is missed. False when it cannot start.
pub(super) fn reader() -> bool {
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        let started = std::thread::Builder::new()
            .name("ostra-decoy-log".into())
            .spawn(move || read_forever(tx));
        started.is_ok() && rx.recv_timeout(Duration::from_secs(10)).unwrap_or(false)
    })
}

/// Runs `log stream` and reports each decoy's first refusal, starting it again if it exits.
fn read_forever(ready: std::sync::mpsc::Sender<bool>) {
    let mut ready = Some(ready);
    loop {
        let child = std::process::Command::new("/usr/bin/log")
            .args(["stream", "--style", "ndjson", "--predicate", PREDICATE])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            if let Some(tx) = ready.take() {
                let _ = tx.send(false);
            }
            return;
        };
        let Some(out) = child.stdout.take() else {
            return;
        };
        let mut attached = false;
        for line in std::io::BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            if !attached {
                // `log stream` prints its filter once it is attached.
                attached = true;
                if let Some(tx) = ready.take() {
                    let _ = tx.send(true);
                }
            }
            if let Some(tag) = tag_of(&line) {
                let hit = registry()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&tag);
                if let Some((path, on)) = hit {
                    on(path);
                }
            }
        }
        let _ = child.wait();
        if let Some(tx) = ready.take() {
            // It exited before attaching: not an admin account, or no log access at all.
            let _ = tx.send(false);
            return;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The decoy message in one `log stream` record, when the kernel wrote it.
pub(super) fn tag_of(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("processID")?.as_u64()? != 0 || v.get("processImagePath")?.as_str()? != "/kernel" {
        return None;
    }
    let msg = v.get("eventMessage")?.as_str()?;
    let at = msg.find(TAG_PREFIX)?;
    let tag: String = msg[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '.')
        .collect();
    Some(tag)
}

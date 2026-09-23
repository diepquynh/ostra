//! PTY sessions for harness executions. A `vt100` screen mirrors the terminal so a browser that
//! attaches mid-run is sent the current screen, and while no browser is attached Ostra answers
//! the terminal queries TUIs send at startup (cursor position, device attributes, colors), which
//! would otherwise leave them waiting.

use crate::launch::LaunchPlan;
use ostra_core::ExecutionId;
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::watch;

pub const DEFAULT_COLS: u16 = 120;
pub const DEFAULT_ROWS: u16 = 40;
const SCROLLBACK: usize = 2000;

/// Variables that tie a process to the harness session Ostra itself was started from. A child that
/// inherits them runs as that session's nested child: Claude Code does not persist such a session,
/// so it could never be resumed.
pub const PARENT_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CODEX_THREAD_ID",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
    "GROK_SESSION_ID",
    "ANTIGRAVITY_AGENT",
];

type OutputSink = Box<dyn Fn(&[u8]) + Send + Sync>;

pub struct PtySession {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    pid: Option<u32>,
    screen: Mutex<vt100::Parser>,
    viewers: AtomicUsize,
    last_output: Mutex<Instant>,
    exit: watch::Receiver<Option<ExitInfo>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<u32>,
    pub at: Instant,
}

impl std::fmt::Debug for PtySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtySession")
            .field("pid", &self.pid)
            .finish()
    }
}

impl PtySession {
    /// Spawn `plan` in a new PTY. `on_output` receives every byte the program writes.
    pub fn spawn(
        plan: &LaunchPlan,
        cols: u16,
        rows: u16,
        on_output: OutputSink,
    ) -> std::io::Result<Arc<Self>> {
        let io = |e: anyhow_like::Error| std::io::Error::other(e.0);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io(e.into()))?;
        let mut cmd = CommandBuilder::new(&plan.program);
        cmd.args(&plan.args);
        cmd.cwd(&plan.cwd);
        for (k, v) in std::env::vars_os() {
            cmd.env(k, v);
        }
        for k in PARENT_SESSION_ENV {
            cmd.env_remove(k);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        for (k, v) in &plan.env {
            cmd.env(k, v);
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| io(e.into()))?;
        drop(pair.slave);
        let pid = child.process_id();
        let reader = pair.master.try_clone_reader().map_err(|e| io(e.into()))?;
        let writer = pair.master.take_writer().map_err(|e| io(e.into()))?;
        let (exit_tx, exit_rx) = watch::channel(None);
        let session = Arc::new(PtySession {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            pid,
            screen: Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)),
            viewers: AtomicUsize::new(0),
            last_output: Mutex::new(Instant::now()),
            exit: exit_rx,
        });
        let reading = session.clone();
        std::thread::Builder::new()
            .name(format!("pty-read-{}", pid.unwrap_or(0)))
            .spawn(move || reading.read_loop(reader, on_output))?;
        std::thread::Builder::new()
            .name(format!("pty-wait-{}", pid.unwrap_or(0)))
            .spawn(move || {
                let code = child.wait().ok().map(|s| s.exit_code());
                let _ = exit_tx.send(Some(ExitInfo {
                    code,
                    at: Instant::now(),
                }));
            })?;
        Ok(session)
    }

    fn read_loop(self: Arc<Self>, mut reader: Box<dyn Read + Send>, on_output: OutputSink) {
        let mut buf = [0u8; 16 * 1024];
        let mut carry: Vec<u8> = vec![];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let chunk = &buf[..n];
            *self.last_output.lock() = Instant::now();
            self.screen.lock().process(chunk);
            on_output(chunk);
            if self.viewers.load(Ordering::SeqCst) == 0 {
                carry.extend_from_slice(chunk);
                let (replies, keep) = {
                    let screen = self.screen.lock();
                    let (row, col) = screen.screen().cursor_position();
                    answer_queries(&carry, row + 1, col + 1)
                };
                carry.drain(..carry.len() - keep);
                if !replies.is_empty() {
                    let _ = self.write(&replies);
                }
            } else {
                carry.clear();
            }
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut w = self.writer.lock();
        w.write_all(bytes)?;
        w.flush()
    }

    /// Type a line of text as if a person entered it, then press Enter.
    pub fn type_line(&self, text: &str) -> std::io::Result<()> {
        // Bracketed paste keeps TUIs from treating embedded newlines as submits.
        self.write(format!("\x1b[200~{text}\x1b[201~").as_bytes())?;
        std::thread::sleep(Duration::from_millis(150));
        self.write(b"\r")
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.master.lock().resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.screen.lock().screen_mut().set_size(rows, cols);
    }

    /// Escape sequences that redraw the current screen, for a browser that attaches now.
    pub fn snapshot(&self) -> Vec<u8> {
        let screen = self.screen.lock();
        let mut out = b"\x1b[H\x1b[2J".to_vec();
        out.extend(screen.screen().contents_formatted());
        out
    }

    /// Visible text, for recognizing login and trust prompts.
    pub fn screen_text(&self) -> String {
        self.screen.lock().screen().contents()
    }

    pub fn attach(&self) {
        self.viewers.fetch_add(1, Ordering::SeqCst);
    }

    pub fn detach(&self) {
        let _ = self
            .viewers
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
                Some(v.saturating_sub(1))
            });
    }

    pub fn idle_for(&self) -> Duration {
        self.last_output.lock().elapsed()
    }

    pub fn exit_info(&self) -> Option<ExitInfo> {
        *self.exit.borrow()
    }

    pub async fn wait_exit(&self) -> ExitInfo {
        let mut rx = self.exit.clone();
        loop {
            if let Some(info) = *rx.borrow() {
                return info;
            }
            if rx.changed().await.is_err() {
                return ExitInfo {
                    code: None,
                    at: Instant::now(),
                };
            }
        }
    }

    /// Signal the process group, then kill it if it is still alive after `grace`.
    pub async fn terminate(&self, grace: Duration) {
        if self.exit_info().is_some() {
            return;
        }
        self.signal(libc::SIGTERM);
        if tokio::time::timeout(grace, self.wait_exit()).await.is_err() {
            self.signal(libc::SIGKILL);
            let _ = tokio::time::timeout(Duration::from_secs(2), self.wait_exit()).await;
        }
    }

    fn signal(&self, sig: i32) {
        if let Some(pid) = self.pid.and_then(|p| i32::try_from(p).ok()) {
            // SAFETY: plain kill(2) on the child's process group; the PTY child leads its own
            // session, so -pid addresses it and its descendants only.
            unsafe {
                libc::kill(-pid, sig);
                libc::kill(pid, sig);
            }
        }
    }
}

mod anyhow_like {
    pub struct Error(pub String);
    impl<E: std::fmt::Display> From<E> for Error {
        fn from(e: E) -> Self {
            Error(e.to_string())
        }
    }
}

/// Replies to the terminal queries found in `buf`, and how many trailing bytes to keep because
/// they may start a sequence the next read completes.
pub fn answer_queries(buf: &[u8], row: u16, col: u16) -> (Vec<u8>, usize) {
    let mut out = vec![];
    let mut i = 0;
    let mut keep_from = buf.len();
    while i < buf.len() {
        if buf[i] != 0x1b {
            i += 1;
            continue;
        }
        let rest = &buf[i..];
        match parse_query(rest) {
            Parsed::Query(len, reply) => {
                if let Some(r) = reply {
                    out.extend(reply_bytes(r, row, col));
                }
                i += len;
            }
            Parsed::Other(len) => i += len.max(1),
            Parsed::Incomplete => {
                keep_from = i;
                break;
            }
        }
    }
    let keep = buf.len() - keep_from;
    (out, keep.min(64))
}

#[derive(Debug, PartialEq)]
enum Reply {
    CursorPosition,
    Status,
    PrimaryDa,
    SecondaryDa,
    Foreground,
    Background,
    KittyKeyboard,
    XtVersion,
    Mode(String),
}

enum Parsed {
    Query(usize, Option<Reply>),
    Other(usize),
    Incomplete,
}

fn parse_query(s: &[u8]) -> Parsed {
    if s.len() < 2 {
        return Parsed::Incomplete;
    }
    match s[1] {
        b'[' => {
            let mut j = 2;
            while j < s.len() && !(0x40..=0x7e).contains(&s[j]) {
                j += 1;
            }
            if j >= s.len() {
                return if j - 2 > 32 {
                    Parsed::Other(j)
                } else {
                    Parsed::Incomplete
                };
            }
            let params = &s[2..j];
            let fin = s[j];
            let len = j + 1;
            let reply = match (params, fin) {
                (b"6", b'n') => Some(Reply::CursorPosition),
                (b"5", b'n') => Some(Reply::Status),
                (b"" | b"0", b'c') => Some(Reply::PrimaryDa),
                (b">" | b">0", b'c') => Some(Reply::SecondaryDa),
                (b"?", b'u') => Some(Reply::KittyKeyboard),
                (b">" | b">0", b'q') => Some(Reply::XtVersion),
                (p, b'p') if p.starts_with(b"?") && p.ends_with(b"$") => Some(Reply::Mode(
                    String::from_utf8_lossy(&p[1..p.len() - 1]).into_owned(),
                )),
                _ => None,
            };
            match reply {
                Some(r) => Parsed::Query(len, Some(r)),
                None => Parsed::Other(len),
            }
        }
        b']' => {
            let mut j = 2;
            let mut end = None;
            while j < s.len() {
                if s[j] == 0x07 {
                    end = Some((j, j + 1));
                    break;
                }
                if s[j] == 0x1b && j + 1 < s.len() && s[j + 1] == b'\\' {
                    end = Some((j, j + 2));
                    break;
                }
                j += 1;
            }
            let Some((body_end, len)) = end else {
                return if s.len() > 256 {
                    Parsed::Other(s.len())
                } else {
                    Parsed::Incomplete
                };
            };
            match &s[2..body_end] {
                b"10;?" => Parsed::Query(len, Some(Reply::Foreground)),
                b"11;?" => Parsed::Query(len, Some(Reply::Background)),
                _ => Parsed::Other(len),
            }
        }
        _ => Parsed::Other(2),
    }
}

fn reply_bytes(r: Reply, row: u16, col: u16) -> Vec<u8> {
    match r {
        Reply::CursorPosition => format!("\x1b[{row};{col}R").into_bytes(),
        Reply::Status => b"\x1b[0n".to_vec(),
        Reply::PrimaryDa => b"\x1b[?62;22c".to_vec(),
        Reply::SecondaryDa => b"\x1b[>1;10;0c".to_vec(),
        Reply::Foreground => b"\x1b]10;rgb:d0d0/d0d0/d0d0\x1b\\".to_vec(),
        Reply::Background => b"\x1b]11;rgb:1c1c/1c1c/1c1c\x1b\\".to_vec(),
        Reply::KittyKeyboard => b"\x1b[?0u".to_vec(),
        Reply::XtVersion => b"\x1bP>|ostra\x1b\\".to_vec(),
        Reply::Mode(m) => format!("\x1b[?{m};0$y").into_bytes(),
    }
}

/// Live PTYs by execution id, so the server can forward browser input, resizes, and attaches.
#[derive(Debug, Default)]
pub struct PtyRegistry {
    map: Mutex<HashMap<ExecutionId, Arc<PtySession>>>,
}

impl PtyRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(PtyRegistry::default())
    }

    pub fn insert(&self, id: ExecutionId, pty: Arc<PtySession>) {
        self.map.lock().insert(id, pty);
    }

    pub fn get(&self, id: &ExecutionId) -> Option<Arc<PtySession>> {
        self.map.lock().get(id).cloned()
    }

    pub fn remove(&self, id: &ExecutionId) -> Option<Arc<PtySession>> {
        self.map.lock().remove(id)
    }

    pub fn contains(&self, id: &ExecutionId) -> bool {
        self.map.lock().contains_key(id)
    }

    pub fn input(&self, id: &ExecutionId, bytes: &[u8]) -> bool {
        self.get(id).is_some_and(|p| p.write(bytes).is_ok())
    }

    pub fn resize(&self, id: &ExecutionId, cols: u16, rows: u16) -> bool {
        self.get(id).map(|p| p.resize(cols, rows)).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_startup_queries() {
        let (r, keep) = answer_queries(b"hello\x1b[6nworld\x1b[c\x1b]11;?\x07", 3, 7);
        assert_eq!(keep, 0);
        assert_eq!(
            r,
            b"\x1b[3;7R\x1b[?62;22c\x1b]11;rgb:1c1c/1c1c/1c1c\x1b\\".to_vec()
        );
        let (r, keep) = answer_queries(b"x\x1b[?1049h\x1b[31m", 1, 1);
        assert!(r.is_empty());
        assert_eq!(keep, 0);
        let (r, keep) = answer_queries(b"x\x1b[", 1, 1);
        assert!(r.is_empty());
        assert_eq!(keep, 2);
        let (r, _) = answer_queries(b"\x1b[?2026$p\x1b[?u", 1, 1);
        assert_eq!(r, b"\x1b[?2026;0$y\x1b[?0u".to_vec());
    }

    #[tokio::test]
    async fn runs_a_program_in_a_pty() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = LaunchPlan {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf 'ready\\n'; read line; printf 'got %s\\n' \"$line\"; exit 3".into(),
            ],
            env: vec![("OSTRA_TEST".into(), "1".into())],
            cwd: tmp.path().to_path_buf(),
            files: vec![],
            links: vec![],
            session_id: None,
        };
        let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
        let sink = seen.clone();
        let pty = PtySession::spawn(
            &plan,
            80,
            24,
            Box::new(move |b| sink.lock().extend_from_slice(b)),
        )
        .unwrap();
        for _ in 0..50 {
            if pty.screen_text().contains("ready") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        pty.write(b"abc\r").unwrap();
        let exit = tokio::time::timeout(Duration::from_secs(5), pty.wait_exit())
            .await
            .unwrap();
        assert_eq!(exit.code, Some(3));
        let out = String::from_utf8_lossy(&seen.lock()).into_owned();
        assert!(out.contains("got abc"), "{out}");
        assert!(pty.snapshot().starts_with(b"\x1b[H\x1b[2J"));
    }

    #[tokio::test]
    async fn terminate_kills_the_group() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = LaunchPlan {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "trap '' TERM; sleep 60 & wait".into()],
            env: vec![],
            cwd: tmp.path().to_path_buf(),
            files: vec![],
            links: vec![],
            session_id: None,
        };
        let pty = PtySession::spawn(&plan, 80, 24, Box::new(|_| {})).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let t = Instant::now();
        pty.terminate(Duration::from_millis(300)).await;
        assert!(pty.exit_info().is_some());
        assert!(t.elapsed() < Duration::from_secs(4));
    }
}

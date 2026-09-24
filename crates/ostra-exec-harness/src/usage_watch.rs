//! Follows a harness session file while its execution runs and reports the usage it adds up to,
//! so harness spend shows while the run is live, as native spend does.

use crate::live::LiveExecution;
use crate::transcript::{self, TranscriptReader};
use notify::{RecursiveMode, Watcher};
use ostra_core::HarnessKind;
use ostra_core::exec::{ExecutionDelta, ExecutionHost, Usage};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How often to look for the session file until it exists: its path depends on a session id
/// the harness may report late.
const LOCATE_EVERY: Duration = Duration::from_secs(2);
/// A turn writes many lines in a burst; one read covers them.
const SETTLE: Duration = Duration::from_millis(500);
/// Used only when the platform watcher cannot be created.
const POLL_EVERY: Duration = Duration::from_secs(5);

/// Run until `stop` fires. Await it after cancelling, so no usage lands after the final result.
pub async fn follow(
    harness: HarnessKind,
    home: PathBuf,
    live: Arc<LiveExecution>,
    host: Arc<dyn ExecutionHost>,
    stop: CancellationToken,
) {
    if harness == HarnessKind::Agy {
        return;
    }
    let path = loop {
        let s = live.snapshot();
        if let Some(p) = transcript::locate(harness, s.transcript_path.as_deref(), s.session_id.as_deref(), &home) {
            break p;
        }
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(LOCATE_EVERY) => {}
        }
    };
    // Watch the directory rather than the file, so a replaced file is still seen.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(1);
    let name = path.file_name().map(ToOwned::to_owned);
    let watcher = path.parent().map(|dir| {
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok_and(|e| e.paths.iter().any(|p| p.file_name() == name.as_deref())) {
                let _ = tx.try_send(());
            }
        })
        .and_then(|mut w| w.watch(dir, RecursiveMode::NonRecursive).map(|_| w))
    });
    let _watcher = match watcher {
        Some(Ok(w)) => Some(w),
        Some(Err(e)) => {
            tracing::warn!(path = %path.display(), "watching the harness session file failed, polling it instead: {e}");
            None
        }
        None => None,
    };
    let watching = _watcher.is_some();
    let mut reader = TranscriptReader::new(harness, path);
    let mut sent: Option<Usage> = None;
    loop {
        if reader.poll() {
            let mut usage = reader.facts().usage;
            usage.tool_calls = live.snapshot().tool_calls;
            if sent != Some(usage) {
                host.emit(ExecutionDelta::Usage { usage });
                sent = Some(usage);
            }
        }
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = rx.recv(), if watching => {}
            _ = tokio::time::sleep(POLL_EVERY), if !watching => {}
        }
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(SETTLE) => {}
        }
        while rx.try_recv().is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::LiveRegistry;
    use ostra_core::policy::{PermissionAnswer, RuleRef, ToolCall};
    use ostra_core::{AgentName, ExecutionId};
    use parking_lot::Mutex;
    use std::io::Write;

    #[derive(Default)]
    struct Host(Mutex<Vec<Usage>>);

    #[async_trait::async_trait]
    impl ExecutionHost for Host {
        fn emit(&self, delta: ExecutionDelta) {
            if let ExecutionDelta::Usage { usage } = delta {
                self.0.lock().push(usage);
            }
        }
        async fn ask_permission(&self, _: &ToolCall, _: &str, _: &RuleRef) -> PermissionAnswer {
            PermissionAnswer::Deny
        }
    }

    async fn until(host: &Host, what: impl Fn(&[Usage]) -> bool) {
        for _ in 0..100 {
            if what(&host.0.lock()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out; saw {:?}", host.0.lock());
    }

    #[tokio::test]
    async fn reports_usage_as_the_session_file_grows() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("s.jsonl");
        let line = |id: &str| format!("{{\"type\":\"assistant\",\"message\":{{\"id\":\"{id}\",\"model\":\"claude-sonnet-5\",\"content\":[],\"usage\":{{\"input_tokens\":0,\"output_tokens\":100}}}}}}\n");
        std::fs::write(&file, line("a")).unwrap();
        let live = LiveRegistry::new().register(ExecutionId::new(), AgentName::Implementer, HarnessKind::Claude);
        live.note_session(None, Some(file.clone()));
        let host = Arc::new(Host::default());
        let stop = CancellationToken::new();
        let task = tokio::spawn(follow(HarnessKind::Claude, tmp.path().into(), live, host.clone(), stop.clone()));
        until(&host, |u| u.last().is_some_and(|u| u.output_tokens == 100)).await;
        std::fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(line("b").as_bytes()).unwrap();
        until(&host, |u| u.last().is_some_and(|u| u.output_tokens == 200)).await;
        stop.cancel();
        task.await.unwrap();
        let n = host.0.lock().len();
        std::fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(line("c").as_bytes()).unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(host.0.lock().len(), n, "nothing is reported after stop");
    }
}

//! `/ws`: one socket per tab, multiplexed by channel (HANDOVER 13).

use crate::app::{App, HubMsg};
use crate::code::HintAsk;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, header};
use axum::response::Response;
use ostra_core::api::{ClientMsg, ServerMsg};
use ostra_core::ids::ExecutionId;
use ostra_core::paths;
use ostra_engine::EngineNotice;
use ostra_exec_harness::PtyRegistry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::task::AbortHandle;

/// Largest client message. Terminal pastes are the big ones.
const MAX_MESSAGE: usize = 1024 * 1024;
/// Largest `term_input`, so one paste cannot fill the PTY's write queue on its own.
const MAX_TERM_INPUT: usize = 64 * 1024;
const MAX_CHANNELS: usize = 256;
/// Binary frames queued for one socket; a slow socket makes its PTY streams lag and resync.
const TERM_QUEUE: usize = 64;
/// How often an open socket re-checks its sign-in, so an expired cookie or a revoke from the CLI
/// disconnects it. A revoke from this server closes it at once.
const RECHECK: Duration = crate::auth::RECHECK;

pub async fn handler(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let cookie =
        crate::auth::cookie_value(headers.get(header::COOKIE).and_then(|h| h.to_str().ok()))
            .unwrap_or_default();
    upgrade
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| run(app, socket, cookie))
}

/// Channels a notice goes to, and the frame to send.
type Routed = (Vec<String>, ServerMsg);

fn route(msg: &HubMsg) -> Routed {
    match &msg.notice {
        EngineNotice::Event { session, stored } => (
            vec![format!("session:{session}")],
            ServerMsg::SessionEvent {
                session: session.clone(),
                seq: stored.seq,
                at: stored.at,
                event: stored.event.clone(),
            },
        ),
        EngineNotice::Delta { execution, item } => (
            vec![format!("execution:{execution}")],
            ServerMsg::ExecutionDelta {
                execution: execution.clone(),
                seq: item.seq,
                at: item.at,
                delta: item.delta.clone(),
            },
        ),
        EngineNotice::ExecutionStatus { execution, status } => (
            vec![format!("execution:{execution}")],
            ServerMsg::ExecutionStatus {
                execution: execution.clone(),
                status: *status,
            },
        ),
        EngineNotice::SessionUpdated { summary } => (
            vec![
                format!("session:{}", summary.id),
                format!("workspace:{}", msg.workspace),
                "home".into(),
            ],
            ServerMsg::SessionUpdated {
                summary: summary.clone(),
            },
        ),
        EngineNotice::ProjectsChanged => (
            vec![format!("workspace:{}", msg.workspace)],
            ServerMsg::WorkspaceUpdated {
                workspace: msg.workspace.clone(),
            },
        ),
    }
}

fn frame(execution: &str, bytes: &[u8]) -> Vec<u8> {
    let id = execution.as_bytes();
    let n = id.len().min(255);
    let mut out = Vec::with_capacity(1 + n + bytes.len());
    out.push(n as u8);
    out.extend_from_slice(&id[..n]);
    out.extend_from_slice(bytes);
    out
}

/// The saved terminal of a harness execution with no live PTY, replayed read-only.
pub fn stored_transcript(app: &App, id: &ExecutionId) -> Option<Vec<u8>> {
    let w = crate::api::ws_of_execution(app, id).ok()?;
    let session = w.db.get_execution(id).ok()??.session?;
    let root = paths::session_root(&w.root, session.as_str());
    ostra_exec_harness::read_transcript(&paths::terminal_transcript(&root, id.as_str()))
}

/// Feed `term:<id>` to one socket: the live screen and everything after it, a fresh screen after
/// falling behind, and the stored transcript when no PTY is running. Waits for a run that has not
/// started its PTY yet.
async fn stream_terminal(
    app: Arc<App>,
    ptys: Arc<PtyRegistry>,
    id: ExecutionId,
    out: mpsc::Sender<Vec<u8>>,
) {
    let mut inserted = ptys.inserted();
    let mut first = true;
    loop {
        inserted.borrow_and_update();
        let Some(pty) = ptys.get(&id) else {
            if std::mem::take(&mut first) {
                let (app, tid) = (app.clone(), id.clone());
                let stored = tokio::task::spawn_blocking(move || stored_transcript(&app, &tid))
                    .await
                    .ok()
                    .flatten();
                if let Some(bytes) = stored
                    && out.send(frame(id.as_str(), &bytes)).await.is_err()
                {
                    return;
                }
            }
            if inserted.changed().await.is_err() {
                return;
            }
            continue;
        };
        first = false;
        let (screen, mut rx) = pty.stream();
        drop(pty);
        if out.send(frame(id.as_str(), &screen)).await.is_err() {
            return;
        }
        // Ends on Lagged, to resync from a fresh screen, or on Closed, to wait for another PTY.
        while let Ok(chunk) = rx.recv().await {
            if out.send(frame(id.as_str(), &chunk)).await.is_err() {
                return;
            }
        }
    }
}

/// Stops a socket's terminal streams when the socket ends, however it ends.
#[derive(Default)]
struct Streams(HashMap<String, AbortHandle>);

impl Streams {
    fn stop(&mut self, channel: &str) {
        if let Some(h) = self.0.remove(channel) {
            h.abort();
        }
    }
}

impl Streams {
    /// Run `task` under `key`, stopping the one it replaces.
    fn replace(&mut self, key: &str, task: AbortHandle) {
        self.stop(key);
        self.0.insert(key.to_string(), task);
    }
}

/// Keys of a socket's in-flight hint requests in `Streams`. A newer request of a kind stops the
/// older one, which then never answers.
const HINT_COMPLETE: &str = "hint:complete";
const HINT_SIGNATURE: &str = "hint:signature";
const HINT_NAVIGATE: &str = "hint:navigate";
/// Hint answers queued for one socket.
const HINT_QUEUE: usize = 8;

fn split<T>(r: Result<T, crate::api::ApiErr>) -> (Option<T>, Option<String>) {
    match r {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e.message().to_string())),
    }
}

impl Drop for Streams {
    fn drop(&mut self) {
        self.0.values().for_each(AbortHandle::abort);
    }
}

async fn send(socket: &mut WebSocket, msg: &ServerMsg) -> bool {
    match serde_json::to_string(msg) {
        Ok(text) => socket.send(Message::Text(text.into())).await.is_ok(),
        Err(_) => true,
    }
}

async fn error(socket: &mut WebSocket, message: &str) -> bool {
    send(
        socket,
        &ServerMsg::Error {
            message: message.into(),
        },
    )
    .await
}

async fn run(app: Arc<App>, mut socket: WebSocket, cookie: String) {
    let mut rx = app.hub.subscribe();
    let mut pushed = app.push.subscribe();
    let mut channels: HashSet<String> = HashSet::new();
    let mut streams = Streams::default();
    let (term_tx, mut term_rx) = mpsc::channel::<Vec<u8>>(TERM_QUEUE);
    let (hint_tx, mut hint_rx) = mpsc::channel::<ServerMsg>(HINT_QUEUE);
    let mut recheck = tokio::time::interval(RECHECK);
    recheck.tick().await;
    let mut revoked = app.auth.subscribe_revoked();
    let mine = crate::auth::cookie_hash(&cookie);
    let signed_out = CloseFrame {
        code: 4401,
        reason: "Signed out".into(),
    };
    loop {
        tokio::select! {
            _ = recheck.tick() => {
                if !app.auth.check_cookie(&cookie) {
                    let _ = socket.send(Message::Close(Some(signed_out.clone()))).await;
                    break;
                }
            }
            r = revoked.recv() => {
                let hit = match r {
                    Ok(h) => h == mine,
                    // Missed revokes: ask the registry instead.
                    Err(broadcast::error::RecvError::Lagged(_)) => !app.auth.check_cookie(&cookie),
                    Err(broadcast::error::RecvError::Closed) => false,
                };
                if hit {
                    let _ = socket.send(Message::Close(Some(signed_out.clone()))).await;
                    break;
                }
            }
            Some(bytes) = term_rx.recv() => {
                if socket.send(Message::Binary(bytes.into())).await.is_err() { break }
            }
            Some(msg) = hint_rx.recv() => {
                if !send(&mut socket, &msg).await { break }
            }
            p = pushed.recv() => {
                match p {
                    Ok(p) => {
                        if p.channels.iter().any(|c| channels.contains(c)) && !send(&mut socket, &p.msg).await { break }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if !error(&mut socket, "lagged").await { break }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.recv() => {
                let Some(Ok(msg)) = incoming else { break };
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => break,
                    _ => continue,
                };
                let Ok(client) = serde_json::from_str::<ClientMsg>(&text) else {
                    if !error(&mut socket, "Unreadable message.").await { break }
                    continue;
                };
                match client {
                    ClientMsg::Subscribe { channel } => {
                        if !channels.contains(&channel) && channels.len() >= MAX_CHANNELS {
                            if !error(&mut socket, "Too many subscriptions on one connection. Unsubscribe from channels you no longer show.").await { break }
                            continue;
                        }
                        channels.insert(channel.clone());
                        if let Some(id) = channel.strip_prefix("term:") {
                            // A repeated subscribe restarts the stream, which resends the screen.
                            streams.stop(&channel);
                            let task = tokio::spawn(stream_terminal(app.clone(), app.shared.harness.ptys(), ExecutionId::from(id), term_tx.clone()));
                            streams.0.insert(channel.clone(), task.abort_handle());
                        }
                        if channel == "home" {
                            let statuses = app.shared.env.read().harnesses.clone();
                            if !send(&mut socket, &ServerMsg::HarnessStatus { statuses }).await { break }
                        }
                        if !send(&mut socket, &ServerMsg::Subscribed { channel }).await { break }
                    }
                    ClientMsg::Unsubscribe { channel } => {
                        streams.stop(&channel);
                        channels.remove(&channel);
                    }
                    ClientMsg::TermInput { execution, data } => {
                        if data.len() > MAX_TERM_INPUT {
                            if !error(&mut socket, "Terminal input is limited to 64 KiB per message. Paste in smaller parts.").await { break }
                            continue;
                        }
                        if let Some(pty) = app.shared.harness.ptys().get(&execution)
                            && pty.write(data.as_bytes()).is_err()
                            && !error(&mut socket, "The harness is not reading terminal input right now, so the input was dropped.").await
                        {
                            break;
                        }
                    }
                    ClientMsg::CodeComplete { id, workspace, key, path, text, line, col, trigger, retrigger } => {
                        let ask = HintAsk { path, text, line, col, trigger, retrigger };
                        let (app, tx) = (app.clone(), hint_tx.clone());
                        let task = tokio::spawn(async move {
                            let r = match crate::api::ws(&app, workspace.as_str()) {
                                Ok(w) => app.code.complete(&app, &w, &key, ask).await,
                                Err(e) => Err(e),
                            };
                            let (result, error) = split(r);
                            let _ = tx.send(ServerMsg::CodeCompletion { id, result, error }).await;
                        });
                        streams.replace(HINT_COMPLETE, task.abort_handle());
                    }
                    ClientMsg::CodeSignature { id, workspace, key, path, text, line, col, trigger, retrigger } => {
                        let ask = HintAsk { path, text, line, col, trigger, retrigger };
                        let (app, tx) = (app.clone(), hint_tx.clone());
                        let task = tokio::spawn(async move {
                            let r = match crate::api::ws(&app, workspace.as_str()) {
                                Ok(w) => app.code.signature(&app, &w, &key, ask).await,
                                Err(e) => Err(e),
                            };
                            let (result, error) = split(r);
                            let _ = tx.send(ServerMsg::CodeSignatureHelp { id, result: result.flatten(), error }).await;
                        });
                        streams.replace(HINT_SIGNATURE, task.abort_handle());
                    }
                    ClientMsg::CodeNavigate { id, workspace, key, path, text, line, col, target } => {
                        let ask = HintAsk { path, text, line, col, trigger: None, retrigger: false };
                        let (app, tx) = (app.clone(), hint_tx.clone());
                        let task = tokio::spawn(async move {
                            let r = match crate::api::ws(&app, workspace.as_str()) {
                                Ok(w) => app.code.navigate(&app, &w, &key, ask, target).await,
                                Err(e) => Err(e),
                            };
                            let (result, error) = split(r);
                            let _ = tx.send(ServerMsg::CodeNavigation { id, result: result.flatten(), error }).await;
                        });
                        streams.replace(HINT_NAVIGATE, task.abort_handle());
                    }
                    ClientMsg::TermResize { execution, cols, rows } => {
                        if cols == 0 || rows == 0 || cols > ostra_exec_harness::pty::MAX_COLS || rows > ostra_exec_harness::pty::MAX_ROWS {
                            if !error(&mut socket, "Terminal size is out of range.").await { break }
                            continue;
                        }
                        app.shared.harness.ptys().resize(&execution, cols, rows);
                    }
                }
            }
            notice = rx.recv() => {
                let msg = match notice {
                    Ok(m) => m,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // The page refetches its REST snapshot on this error.
                        if !error(&mut socket, "lagged").await { break }
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let (targets, json) = route(&msg);
                if targets.iter().any(|c| channels.contains(c)) && !send(&mut socket, &json).await {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pty_frame_layout() {
        let f = super::frame("x_1", b"hi");
        assert_eq!(f, vec![3, b'x', b'_', b'1', b'h', b'i']);
    }
}

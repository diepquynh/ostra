//! `/ws`: one socket per tab, multiplexed by channel (HANDOVER 13).

use crate::app::{App, HubMsg};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use ostra_core::api::{ClientMsg, ServerMsg};
use ostra_core::ids::ExecutionId;
use ostra_core::paths;
use ostra_engine::EngineNotice;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::broadcast;

pub async fn handler(State(app): State<Arc<App>>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| run(app, socket))
}

/// Channels a notice goes to, and the frame to send.
type Routed = (Vec<String>, Option<ServerMsg>, Option<(String, Vec<u8>)>);

fn route(msg: &HubMsg) -> Routed {
    match &msg.notice {
        EngineNotice::Event { session, stored } => (
            vec![format!("session:{session}")],
            Some(ServerMsg::SessionEvent { session: session.clone(), seq: stored.seq, at: stored.at, event: stored.event.clone() }),
            None,
        ),
        EngineNotice::Delta { execution, item } => (
            vec![format!("execution:{execution}")],
            Some(ServerMsg::ExecutionDelta { execution: execution.clone(), seq: item.seq, at: item.at, delta: item.delta.clone() }),
            None,
        ),
        EngineNotice::ExecutionStatus { execution, status } => (
            vec![format!("execution:{execution}")],
            Some(ServerMsg::ExecutionStatus { execution: execution.clone(), status: *status }),
            None,
        ),
        EngineNotice::SessionUpdated { summary } => (
            vec![format!("session:{}", summary.id), format!("workspace:{}", msg.workspace), "home".into()],
            Some(ServerMsg::SessionUpdated { summary: summary.clone() }),
            None,
        ),
        EngineNotice::ProjectsChanged => (
            vec![format!("workspace:{}", msg.workspace)],
            Some(ServerMsg::WorkspaceUpdated { workspace: msg.workspace.clone() }),
            None,
        ),
        EngineNotice::Terminal { execution, bytes } => (vec![format!("term:{execution}")], None, Some((execution.to_string(), bytes.clone()))),
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

async fn send(socket: &mut WebSocket, msg: &ServerMsg) -> bool {
    match serde_json::to_string(msg) {
        Ok(text) => socket.send(Message::Text(text.into())).await.is_ok(),
        Err(_) => true,
    }
}

async fn run(app: Arc<App>, mut socket: WebSocket) {
    let mut rx = app.hub.subscribe();
    let mut pushed = app.push.subscribe();
    let mut channels: HashSet<String> = HashSet::new();
    loop {
        tokio::select! {
            p = pushed.recv() => {
                match p {
                    Ok(p) => {
                        if p.channels.iter().any(|c| channels.contains(c)) && !send(&mut socket, &p.msg).await { break }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if !send(&mut socket, &ServerMsg::Error { message: "lagged".into() }).await { break }
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
                    if !send(&mut socket, &ServerMsg::Error { message: "Unreadable message.".into() }).await { break }
                    continue;
                };
                match client {
                    ClientMsg::Subscribe { channel } => {
                        channels.insert(channel.clone());
                        if channel.starts_with("term:") {
                            let id = ostra_core::ids::ExecutionId::from(channel.trim_start_matches("term:"));
                            let backlog = app.shared.harness.backlog(&id).or_else(|| stored_transcript(&app, &id));
                            if let Some(backlog) = backlog
                                && socket.send(Message::Binary(frame(id.as_str(), &backlog).into())).await.is_err()
                            {
                                break;
                            }
                        }
                        if channel == "home" {
                            let statuses = app.shared.env.read().harnesses.clone();
                            if !send(&mut socket, &ServerMsg::HarnessStatus { statuses }).await { break }
                        }
                        if !send(&mut socket, &ServerMsg::Subscribed { channel }).await { break }
                    }
                    ClientMsg::Unsubscribe { channel } => {
                        if let Some(id) = channel.strip_prefix("term:") {
                            app.shared.harness.detach(&ostra_core::ids::ExecutionId::from(id));
                        }
                        channels.remove(&channel);
                    }
                    ClientMsg::TermInput { execution, data } => app.shared.harness.input(&execution, data.as_bytes()),
                    ClientMsg::TermResize { execution, cols, rows } => app.shared.harness.resize(&execution, cols, rows),
                }
            }
            notice = rx.recv() => {
                let msg = match notice {
                    Ok(m) => m,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // The page refetches its REST snapshot on this error.
                        if !send(&mut socket, &ServerMsg::Error { message: "lagged".into() }).await { break }
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let (targets, json, binary) = route(&msg);
                if !targets.iter().any(|c| channels.contains(c)) {
                    continue;
                }
                if let Some(json) = json && !send(&mut socket, &json).await {
                    break;
                }
                if let Some((id, bytes)) = binary && socket.send(Message::Binary(frame(&id, &bytes).into())).await.is_err() {
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

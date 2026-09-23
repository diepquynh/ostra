use crate::ProviderError;
use crate::retry::network_error;
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};

/// JSON payloads of an SSE response, in order. `[DONE]` sentinels and empty data are skipped.
pub(crate) fn json_events(
    resp: reqwest::Response,
) -> impl Stream<Item = Result<serde_json::Value, ProviderError>> + Send + Unpin {
    Box::pin(resp.bytes_stream().eventsource().filter_map(|ev| async move {
        match ev {
            Ok(ev) => {
                let data = ev.data.trim();
                if data.is_empty() || data == "[DONE]" {
                    return None;
                }
                Some(serde_json::from_str::<serde_json::Value>(data).map_err(|e| {
                    ProviderError::Decode(format!("bad event `{}`: {e}", ev.event))
                }))
            }
            Err(eventsource_stream::EventStreamError::Transport(e)) => Some(Err(network_error(e))),
            Err(e) => Some(Err(ProviderError::Decode(e.to_string()))),
        }
    }))
}

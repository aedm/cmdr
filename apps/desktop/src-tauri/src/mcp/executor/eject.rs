//! The `eject` tool: eject an ejectable volume by id.
//!
//! Thin adapter over the typed `file_system::volume::eject::eject` (smart backend
//! / thin frontend). It dispatches no FE action and invents no ack — it calls the
//! backend directly and returns OK (the `connect_to_server` / `indexing` / `queue`
//! precedent, so there is no FE action to ack). Gate `Open`: parity with the one-click Eject button, and
//! the backend refuses honestly while a write op touches the volume (`Busy`) or
//! when the volume isn't ejectable, surfaced as errors rather than false OKs.

use serde_json::{Value, json};

use super::{ToolError, ToolResult};

pub async fn execute_eject(params: &Value) -> ToolResult {
    let volume_id = params
        .get("volumeId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("Missing 'volumeId' parameter"))?;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use crate::file_system::volume::eject::EjectError;

        match crate::file_system::volume::eject::eject(volume_id).await {
            Ok(()) => Ok(json!(format!(
                "OK: Ejecting {volume_id}. The volume disappears once teardown completes."
            ))),
            Err(error) => {
                let reply = ToolError::internal(format!("Couldn't eject {volume_id}: {error}"));
                // A refusal is the one answer an agent can act on, so who holds the drive
                // rides in `data` as the typed value rather than only in the sentence
                // (`no-error-string-match` applies to an agent parsing us too). ❗ The
                // holders keep their own tag: an agent may not read an empty list as
                // "nobody is holding it".
                Err(match error {
                    EjectError::UnmountRefused { holders, .. } => reply.with_data(json!({
                        "outcome": "unmountRefused",
                        "holders": holders,
                    })),
                    _ => reply,
                })
            }
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = volume_id;
        Err(ToolError::internal("Eject isn't supported on this platform"))
    }
}

use crate::{inference_full, storage::ApiError, AppState};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn reason(error: ApiError) -> String {
    error.1 .0["error"]
        .as_str()
        .unwrap_or("GPU status unavailable")
        .to_owned()
}
fn outcome(status: &Value, cancelled: bool) -> Option<Result<(), String>> {
    let phase = status["phase"].as_str().unwrap_or("unknown");
    match phase {
        "complete" if !cancelled => Some(Ok(())),
        "complete" | "cancelled" if cancelled => Some(Err("cancelled".into())),
        "failed" | "cancelled" | "interrupted" => Some(Err(format!(
            "GPU {phase}: {}",
            status["error"].as_str().unwrap_or(phase)
        ))),
        "queued" | "running" | "loading_model" => None,
        _ => Some(Err(format!("Unrecognized GPU stage: {phase}"))),
    }
}
async fn cancel(app: &Arc<AppState>, project: &str, job: &str) {
    if let Ok(request) = serde_json::from_value::<inference_full::CancelRequest>(
        json!({"project_id":project,"job_id":job}),
    ) {
        let _ = inference_full::cancel(State(app.clone()), HeaderMap::new(), Json(request)).await;
    }
}
pub(crate) async fn wait(
    app: &Arc<AppState>,
    project: &str,
    job: &str,
    cancelled: impl Fn() -> bool,
    mut update: impl FnMut(&Value) -> Result<(), String>,
) -> Result<String, String> {
    let mut cancel_sent = false;
    let result = async {
        loop {
            if cancelled() && !cancel_sent {
                cancel(app, project, job).await;
                cancel_sent = true;
            }
            let query = serde_json::from_value(json!({"project_id":project,"job_id":job}))
                .map_err(|e| format!("GPU query: {e}"))?;
            let Json(status) = inference_full::status(State(app.clone()), Query(query))
                .await
                .map_err(reason)?;
            update(&status)?;
            if let Some(result) = outcome(&status, cancel_sent || cancelled()) {
                return result.map(|()| job.to_owned());
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    }
    .await;
    if result.is_err() {
        cancel(app, project, job).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn terminal_states_preserve_cancellation_and_reject_unknown_phases() {
        assert_eq!(outcome(&json!({"phase":"complete"}), false), Some(Ok(())));
        assert_eq!(
            outcome(&json!({"phase":"complete"}), true),
            Some(Err("cancelled".into()))
        );
        assert_eq!(outcome(&json!({"phase":"running"}), false), None);
        assert!(
            outcome(&json!({"phase":"failed","error":"device lost"}), false)
                .unwrap()
                .unwrap_err()
                .contains("device lost")
        );
        assert!(outcome(&json!({}), false).unwrap().is_err());
    }
}

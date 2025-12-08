use crate::integration::{TelemetryEvent, build_tenant_ctx};
use crate::server::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

pub async fn serve_sdk(State(_state): State<AppState>) -> impl IntoResponse {
    match std::fs::read_to_string("assets/gui-sdk.js") {
        Ok(script) => {
            let mut resp = Response::new(script);
            resp.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/javascript"),
            );
            resp
        }
        Err(_) => {
            let script = crate::sdk::sdk_script();
            let mut resp = Response::new(script);
            resp.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/javascript"),
            );
            resp
        }
    }
}

pub async fn get_gui_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let domain = super::server::host_from_headers(&headers)
        .unwrap_or_else(|| state.config.default_tenant.clone());
    match state.load_tenant(&domain).await {
        Ok(cfg) => {
            let routes: Vec<serde_json::Value> = cfg
                .features
                .iter()
                .flat_map(|f| {
                    f.manifest.routes.iter().map(|r| {
                        json!({
                            "path": r.path,
                            "authenticated": r.authenticated
                        })
                    })
                })
                .collect();
            let workers: Vec<serde_json::Value> = cfg
                .features
                .iter()
                .flat_map(|f| f.manifest.digital_workers.iter().map(|w| json!(w)))
                .collect();
            let body = json!({
                "tenant": cfg.tenant_did,
                "domain": cfg.domain,
                "routes": routes,
                "workers": workers,
                "skin": cfg.skin.as_ref().map(|s| s.assets.to_string_lossy()),
            });
            Json(body).into_response()
        }
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct WorkerMessageRequest {
    pub worker_id: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub context: WorkerRequestContext,
}

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
pub struct WorkerRequestContext {
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub route: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

pub async fn post_worker_message(
    State(state): State<AppState>,
    Json(body): Json<WorkerMessageRequest>,
) -> impl IntoResponse {
    let tenant_ctx = build_tenant_ctx(
        &state.config.env_id,
        &state.config.default_tenant,
        Some(&state.config.default_team),
        body.context.user_id.as_deref(),
    );
    match state
        .worker_host
        .invoke_worker(tenant_ctx, &body.worker_id, body.payload)
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct TelemetryRequest {
    pub event_type: String,
    pub path: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

pub async fn post_events(
    State(state): State<AppState>,
    Json(body): Json<TelemetryRequest>,
) -> impl IntoResponse {
    crate::integration::set_request_telemetry_ctx(&state.config.default_tenant, None, Some("gui"));
    let event = TelemetryEvent {
        event_type: body.event_type,
        path: body.path,
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        metadata: body.metadata,
    };
    state.telemetry.record_event(event).await;
    StatusCode::ACCEPTED
}

pub async fn clear_cache(State(state): State<AppState>) -> impl IntoResponse {
    state.clear_cache().await;
    StatusCode::NO_CONTENT
}

#[derive(Debug, Deserialize)]
pub struct SessionIssueRequest {
    pub user_id: String,
    #[serde(default)]
    pub team: Option<String>,
}

pub async fn issue_session(
    State(state): State<AppState>,
    Json(body): Json<SessionIssueRequest>,
) -> impl IntoResponse {
    let tenant_ctx = build_tenant_ctx(
        &state.config.env_id,
        &state.config.default_tenant,
        body.team
            .as_deref()
            .or(Some(state.config.default_team.as_str())),
        Some(body.user_id.as_str()),
    );
    let flow_id = greentic_types::FlowId::new("gui").expect("valid flow id");
    match state.session_manager.issue(tenant_ctx, flow_id).await {
        Ok(session) => (
            StatusCode::CREATED,
            [(
                header::SET_COOKIE,
                format!(
                    "greentic_session_id={}; Path=/; HttpOnly",
                    session.session_id
                ),
            )],
            Json(serde_json::json!({
                "session_id": session.session_id,
                "user_id": session.user_id,
            })),
        )
            .into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

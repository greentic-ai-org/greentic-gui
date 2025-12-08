use crate::config::AppConfig;
use crate::server::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect};
use greentic_oauth_client::{Client, ClientBuilder, OwnerKind, StartRequest};
use greentic_oauth_sdk::{TokenValidationConfig, validate_bearer_token};
use serde::Deserialize;

pub async fn start_auth(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> impl IntoResponse {
    match build_oauth_client(&state.config) {
        Ok(client) => {
            let redirect_uri = format!("/auth/{provider}/callback");
            let req = StartRequest {
                env: state.config.env_id.clone(),
                tenant: state.config.default_tenant.clone(),
                provider: provider.clone(),
                team: Some(state.config.default_team.clone()),
                owner_kind: OwnerKind::User,
                owner_id: "gui-user".into(),
                flow_id: "gui-auth".into(),
                scopes: vec!["openid".into(), "profile".into(), "email".into()],
                redirect_uri: Some(redirect_uri),
                visibility: None,
                extra_params: None,
            };
            match client.start(req).await {
                Ok(resp) => (
                    StatusCode::FOUND,
                    [(header::LOCATION, resp.start_url)],
                    "redirecting",
                )
                    .into_response(),
                Err(err) => (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
            }
        }
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub id_token: Option<String>,
}

pub async fn auth_callback(
    State(state): State<AppState>,
    Path(_provider): Path<String>,
    query: axum::extract::Query<CallbackQuery>,
) -> impl IntoResponse {
    let Some(id_token) = query.id_token.clone() else {
        return (StatusCode::BAD_REQUEST, "missing id_token").into_response();
    };

    let cfg = match build_validation_config(&state.config) {
        Ok(cfg) => cfg,
        Err(err) => return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    };

    let (_claims, tenant_ctx) = match validate_bearer_token(&id_token, &cfg).await {
        Ok(res) => res,
        Err(err) => return (StatusCode::UNAUTHORIZED, err.to_string()).into_response(),
    };

    let flow_id = greentic_types::FlowId::new("gui-auth").unwrap();
    match state.session_manager.issue(tenant_ctx, flow_id).await {
        Ok(session) => {
            let cookie = make_session_cookie(&state.config, &session.session_id);
            let mut response = Redirect::to("/").into_response();
            response
                .headers_mut()
                .insert(header::SET_COOKIE, cookie.parse().unwrap());
            response
        }
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

fn build_oauth_client(config: &AppConfig) -> anyhow::Result<Client> {
    let base = config
        .oauth_broker_url
        .clone()
        .ok_or_else(|| anyhow::anyhow!("OAUTH_BROKER_URL not set"))?;
    Ok(ClientBuilder::new().base_url(base)?.build()?)
}

fn build_validation_config(config: &AppConfig) -> anyhow::Result<TokenValidationConfig> {
    let jwks = config
        .oauth_jwks_url
        .clone()
        .ok_or_else(|| anyhow::anyhow!("OAUTH_JWKS_URL not set"))?
        .parse()?;
    let issuer = config
        .oauth_issuer
        .clone()
        .ok_or_else(|| anyhow::anyhow!("OAUTH_ISSUER not set"))?;
    let audience = config
        .oauth_audience
        .clone()
        .ok_or_else(|| anyhow::anyhow!("OAUTH_AUDIENCE not set"))?;

    let mut cfg =
        TokenValidationConfig::new(jwks, issuer, audience).with_env(config.env_id.clone());
    if !config.oauth_required_scopes.is_empty() {
        cfg = cfg.with_required_scopes(config.oauth_required_scopes.clone());
    }
    Ok(cfg)
}

fn make_session_cookie(config: &AppConfig, session_id: &str) -> String {
    let mut cookie = format!(
        "greentic_session_id={}; Path=/; HttpOnly; SameSite=Lax",
        session_id
    );
    if !config.session_ttl.is_zero() {
        cookie.push_str(&format!("; Max-Age={}", config.session_ttl.as_secs()));
    }
    cookie
}

pub async fn logout(State(_state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let mut response = Redirect::to("/").into_response();
    if let Some(session_id) = session_cookie(&headers) {
        // Best-effort: drop cookie; store cleanup is handled elsewhere.
        let expired = format!(
            "greentic_session_id={}; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; HttpOnly; SameSite=Lax",
            session_id
        );
        response
            .headers_mut()
            .insert(header::SET_COOKIE, expired.parse().unwrap());
    }
    response
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .map(|c| c.trim())
                .find_map(|c| c.strip_prefix("greentic_session_id="))
                .map(|s| s.to_string())
        })
}

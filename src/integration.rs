use async_trait::async_trait;
use greentic_session::store::SessionStore;
use greentic_telemetry::init::TelemetryConfig;
use greentic_telemetry::init_telemetry;
use greentic_telemetry::{TelemetryCtx, set_current_telemetry_ctx};
use greentic_types::{
    EnvId, FlowId, SessionCursor, SessionData, SessionKey, TeamId, TenantCtx, TenantId, UserId,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum SessionError {
    #[allow(dead_code)]
    #[error("invalid session")]
    Invalid,
    #[error("session provider unavailable: {0}")]
    Provider(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub tenant_ctx: TenantCtx,
    pub user_id: Option<String>,
}

#[async_trait]
pub trait SessionManager: Send + Sync {
    async fn validate(&self, token: Option<String>) -> Result<Option<SessionInfo>, SessionError>;
    async fn issue(&self, ctx: TenantCtx, flow_id: FlowId) -> Result<SessionInfo, SessionError>;
}

#[allow(dead_code)]
#[derive(Default)]
pub struct StubSessionManager;

#[async_trait]
impl SessionManager for StubSessionManager {
    async fn validate(&self, token: Option<String>) -> Result<Option<SessionInfo>, SessionError> {
        Ok(token.map(|id| SessionInfo {
            session_id: id.clone(),
            tenant_ctx: TenantCtx::new(EnvId::new("dev").unwrap(), TenantId::new("stub").unwrap()),
            user_id: Some("user-stub".to_string()),
        }))
    }

    async fn issue(&self, ctx: TenantCtx, _flow_id: FlowId) -> Result<SessionInfo, SessionError> {
        Ok(SessionInfo {
            session_id: SessionKey::new(uuid::Uuid::new_v4().to_string()).to_string(),
            user_id: ctx.user_id.as_ref().map(|u| u.to_string()),
            tenant_ctx: ctx,
        })
    }
}

#[allow(dead_code)]
/// Real session manager backed by greentic-session InMemory store.
pub struct RealSessionManager {
    store: Arc<dyn SessionStore>,
}

impl RealSessionManager {
    pub fn new(store: Arc<dyn SessionStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl SessionManager for RealSessionManager {
    async fn validate(&self, token: Option<String>) -> Result<Option<SessionInfo>, SessionError> {
        let Some(token) = token else {
            return Ok(None);
        };
        let key = SessionKey::from(token.as_str());
        let data = self
            .store
            .get_session(&key)
            .map_err(|e| SessionError::Provider(e.to_string()))?;
        Ok(data.map(|session| {
            let user = session
                .tenant_ctx
                .user_id
                .as_ref()
                .map(|u| u.to_string())
                .or_else(|| session.tenant_ctx.user.as_ref().map(|u| u.to_string()));
            tracing::debug!(session_id = %key, user = ?user, "validated session");
            SessionInfo {
                session_id: key.to_string(),
                tenant_ctx: session.tenant_ctx.clone(),
                user_id: user,
            }
        }))
    }

    async fn issue(&self, ctx: TenantCtx, flow_id: FlowId) -> Result<SessionInfo, SessionError> {
        let cursor = SessionCursor::new("gui-root");
        let data = SessionData {
            tenant_ctx: ctx.clone(),
            flow_id,
            cursor,
            context_json: "{}".to_string(),
        };
        let key = self
            .store
            .create_session(&ctx, data)
            .map_err(|e| SessionError::Provider(e.to_string()))?;
        tracing::info!(session_id = %key, user = ?ctx.user_id, tenant = %ctx.tenant_id, "issued session");
        Ok(SessionInfo {
            session_id: key.to_string(),
            tenant_ctx: ctx.clone(),
            user_id: ctx.user_id.as_ref().map(|u| u.to_string()),
        })
    }
}

#[allow(dead_code)]
#[async_trait]
pub trait TelemetrySink: Send + Sync {
    async fn record_event(&self, event: TelemetryEvent);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEvent {
    pub event_type: String,
    pub path: String,
    pub timestamp_ms: i64,
    pub metadata: serde_json::Value,
}

#[allow(dead_code)]
pub struct NullTelemetrySink;

#[async_trait]
impl TelemetrySink for NullTelemetrySink {
    async fn record_event(&self, event: TelemetryEvent) {
        tracing::info!(?event, "telemetry event (stub)");
    }
}

/// Telemetry sink that tags events with TelemetryCtx and emits via tracing.
#[derive(Clone, Default)]
#[allow(dead_code)]
pub struct TracingTelemetrySink;

#[async_trait]
impl TelemetrySink for TracingTelemetrySink {
    async fn record_event(&self, event: TelemetryEvent) {
        greentic_telemetry::tasklocal::with_task_local(async {
            tracing::info!(
                event_type = %event.event_type,
                path = %event.path,
                timestamp_ms = event.timestamp_ms,
                metadata = %event.metadata,
                "gui.telemetry.event"
            );
        })
        .await;
    }
}

/// Build a telemetry context from tenant + optional session/provider info.
pub fn make_telemetry_ctx(
    tenant: &str,
    session: Option<&str>,
    provider: Option<&str>,
) -> TelemetryCtx {
    let mut ctx = TelemetryCtx::new(tenant.to_string());
    if let Some(sess) = session {
        ctx = ctx.with_session(sess.to_string());
    }
    if let Some(provider) = provider {
        ctx = ctx.with_provider(provider.to_string());
    }
    ctx
}

/// Attach telemetry context to the current task if available.
pub fn set_request_telemetry_ctx(tenant: &str, session: Option<&str>, provider: Option<&str>) {
    let ctx = make_telemetry_ctx(tenant, session, provider);
    set_current_telemetry_ctx(ctx);
}

#[allow(dead_code)]
pub struct GreenticTelemetrySink;

impl GreenticTelemetrySink {
    pub fn init() {
        let _ = init_telemetry(TelemetryConfig {
            service_name: "greentic-gui".into(),
        });
    }
}

#[async_trait]
impl TelemetrySink for GreenticTelemetrySink {
    async fn record_event(&self, event: TelemetryEvent) {
        info!(
            target: "greentic_gui.telemetry",
            event_type = %event.event_type,
            path = %event.path,
            timestamp = event.timestamp_ms,
            metadata = %event.metadata,
            "gui telemetry event"
        );
    }
}

pub fn build_tenant_ctx(
    env: &str,
    tenant: &str,
    team: Option<&str>,
    user: Option<&str>,
) -> TenantCtx {
    let env_id = EnvId::new(env).unwrap_or_else(|_| EnvId::new("dev").unwrap());
    let tenant_id = TenantId::new(tenant).unwrap_or_else(|_| TenantId::new("tenant").unwrap());
    let mut ctx = TenantCtx::new(env_id, tenant_id);
    if let Some(team) = team {
        ctx = ctx.with_team(Some(
            TeamId::new(team).unwrap_or_else(|_| TeamId::new("gui").unwrap()),
        ));
    }
    if let Some(user) = user {
        ctx = ctx.with_user(Some(
            UserId::new(user).unwrap_or_else(|_| UserId::new("user").unwrap()),
        ));
    }
    ctx
}

// Messaging bus client removed in favor of direct worker host calls.

/// Placeholder hooks for wiring real greentic-* crates.
pub mod real {
    use super::*;

    /// Wraps a real session manager implementation when available.
    #[derive(Clone)]
    #[allow(dead_code)]
    pub struct SessionAdapter {
        inner: Arc<dyn SessionManager>,
    }

    impl SessionAdapter {
        #[allow(dead_code)]
        pub fn new(inner: Arc<dyn SessionManager>) -> Self {
            Self { inner }
        }
    }

    /// Placeholder function to illustrate where greentic-distributor based pack providers would be created.
    #[allow(dead_code)]
    pub fn describe_requirements() {
        warn!(
            "real greentic integrations not linked yet; add greentic-distributor, greentic-session, greentic-telemetry, greentic-messaging crates and wire them to integration::real"
        );
    }
}

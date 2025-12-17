use async_trait::async_trait;
use greentic_interfaces_host::worker::{HostWorkerMessage, HostWorkerRequest, HostWorkerResponse};
use greentic_types::{SecretRequirement, TenantCtx};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::time::sleep;
use tracing::{info, warn};

/// Pluggable backend for invoking workers remotely.
#[async_trait]
pub trait WorkerBackend: Send + Sync {
    async fn invoke(&self, req: HostWorkerRequest) -> anyhow::Result<HostWorkerResponse>;
}

/// Structured error bubbled up when the upstream runtime reports missing secrets.
#[derive(Debug, Clone)]
pub struct MissingSecretsError {
    pub missing_secrets: Vec<SecretRequirement>,
    pub pack_hint: Option<String>,
    pub message: Option<String>,
}

impl std::fmt::Display for MissingSecretsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "missing secrets ({} requirement{})",
            self.missing_secrets.len(),
            if self.missing_secrets.len() == 1 {
                ""
            } else {
                "s"
            }
        )
    }
}

impl std::error::Error for MissingSecretsError {}

/// Stub backend that echoes the payload and marks the response as stubbed.
#[derive(Clone, Default)]
pub struct StubWorkerBackend;

#[async_trait]
impl WorkerBackend for StubWorkerBackend {
    async fn invoke(&self, req: HostWorkerRequest) -> anyhow::Result<HostWorkerResponse> {
        Ok(HostWorkerResponse {
            version: req.version.clone(),
            tenant: req.tenant.clone(),
            worker_id: req.worker_id.clone(),
            timestamp_utc: req.timestamp_utc.clone(),
            messages: vec![HostWorkerMessage {
                kind: "stub".to_string(),
                payload: req.payload.clone(),
            }],
            correlation_id: req.correlation_id.clone(),
            session_id: req.session_id.clone(),
            thread_id: req.thread_id.clone(),
        })
    }
}

/// Optional configuration for a remote worker gateway.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct WorkerGatewayConfig {
    pub base_url: url::Url,
    pub timeout: std::time::Duration,
    pub auth_token: Option<String>,
    pub retries: u32,
    pub backoff_base: std::time::Duration,
}

/// HTTP backend for a remote worker gateway.
#[allow(dead_code)]
#[derive(Clone)]
pub struct HttpWorkerBackend {
    cfg: WorkerGatewayConfig,
    client: reqwest::Client,
}

impl HttpWorkerBackend {
    #[allow(dead_code)]
    pub fn new(cfg: WorkerGatewayConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder().timeout(cfg.timeout).build()?;
        Ok(Self { cfg, client })
    }
}

#[derive(Debug, Deserialize)]
struct MissingSecretsPayload {
    #[serde(default)]
    missing_secrets: Vec<SecretRequirement>,
    #[serde(default)]
    pack_hint: Option<String>,
    #[serde(default)]
    pack_ref: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

async fn parse_missing_secrets_response(resp: reqwest::Response) -> Option<MissingSecretsPayload> {
    let status = resp.status();
    let bytes = resp.bytes().await.ok()?;
    if bytes.is_empty() {
        return None;
    }
    let payload: MissingSecretsPayload = serde_json::from_slice(&bytes).ok()?;
    if payload.missing_secrets.is_empty() {
        return None;
    }
    info!(status = %status, "upstream reported missing secrets");
    Some(payload)
}

#[async_trait]
impl WorkerBackend for HttpWorkerBackend {
    async fn invoke(&self, req: HostWorkerRequest) -> anyhow::Result<HostWorkerResponse> {
        let url = self.cfg.base_url.join("/workers/invoke")?;
        let mut last_err = None;
        for attempt in 0..=self.cfg.retries {
            let mut request = self.client.post(url.clone()).json(&req);
            if let Some(token) = &self.cfg.auth_token {
                request = request.bearer_auth(token);
            }
            match request.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if !status.is_success() {
                        if let Some(missing) = parse_missing_secrets_response(resp).await {
                            return Err(MissingSecretsError {
                                missing_secrets: missing.missing_secrets,
                                pack_hint: missing.pack_hint.or(missing.pack_ref),
                                message: missing.message,
                            }
                            .into());
                        }
                        last_err = Some(anyhow::anyhow!(
                            "worker gateway status {} on attempt {}",
                            status,
                            attempt + 1
                        ));
                    } else {
                        return Ok(resp.json::<HostWorkerResponse>().await?);
                    }
                }
                Err(err) => {
                    last_err = Some(anyhow::anyhow!(
                        "worker gateway request failed on attempt {}: {err}",
                        attempt + 1
                    ));
                }
            }

            if attempt < self.cfg.retries {
                let delay = self.cfg.backoff_base * (attempt + 1);
                sleep(delay).await;
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("worker gateway request failed")))
    }
}

/// Host wrapper that delegates to a backend.
#[derive(Clone)]
pub struct WorkerHost {
    backend: Arc<dyn WorkerBackend>,
}

impl WorkerHost {
    pub fn new(backend: Arc<dyn WorkerBackend>) -> Self {
        Self { backend }
    }

    pub async fn invoke_worker(
        &self,
        tenant_ctx: TenantCtx,
        worker_id: &str,
        payload: Value,
    ) -> anyhow::Result<Value> {
        let span = tracing::info_span!(
            "worker_invoke",
            worker_id = %worker_id,
            tenant = %tenant_ctx.tenant_id,
            session = ?tenant_ctx.session_id
        );
        let _guard = span.enter();
        let req = build_host_worker_request(tenant_ctx, worker_id, payload.clone());
        match self.backend.invoke(req).await {
            Ok(resp) => host_worker_response_to_json(resp),
            Err(err) => {
                if err.downcast_ref::<MissingSecretsError>().is_some() {
                    info!(%worker_id, "worker backend reported missing secrets");
                } else {
                    warn!(%worker_id, ?err, "worker backend call failed");
                }
                Err(err)
            }
        }
    }
}

fn build_host_worker_request(
    tenant_ctx: TenantCtx,
    worker_id: &str,
    payload: Value,
) -> HostWorkerRequest {
    let session_id = tenant_ctx.session_id.clone();
    HostWorkerRequest {
        version: "1.0.0".to_string(),
        tenant: tenant_ctx,
        worker_id: worker_id.to_string(),
        payload,
        timestamp_utc: chrono::Utc::now().to_rfc3339(),
        correlation_id: None,
        session_id,
        thread_id: None,
    }
}

fn host_worker_response_to_json(resp: HostWorkerResponse) -> anyhow::Result<Value> {
    Ok(serde_json::to_value(resp)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_backend_echoes_payload() {
        let backend: Arc<dyn WorkerBackend> = Arc::new(StubWorkerBackend);
        let host = WorkerHost::new(backend);
        let tenant_ctx = greentic_types::TenantCtx::new(
            greentic_types::EnvId::new("dev").unwrap(),
            greentic_types::TenantId::new("tenant").unwrap(),
        );
        let resp = host
            .invoke_worker(tenant_ctx, "worker.echo", serde_json::json!({"x":1}))
            .await
            .unwrap();
        let messages = resp
            .get("messages")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["payload"], serde_json::json!({"x":1}));
    }
}

/// Build a worker backend from env/config. Defaults to stub.
#[allow(dead_code, clippy::items_after_test_module)]
pub fn worker_backend_from_env() -> Arc<dyn WorkerBackend> {
    if let Ok(url) = std::env::var("WORKER_GATEWAY_URL")
        && let Ok(base_url) = url.parse()
    {
        let timeout = std::env::var("WORKER_GATEWAY_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| std::time::Duration::from_secs(5));
        let auth_token = std::env::var("WORKER_GATEWAY_TOKEN").ok();
        let retries = std::env::var("WORKER_GATEWAY_RETRIES")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(2);
        let backoff_base = std::env::var("WORKER_GATEWAY_BACKOFF_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| std::time::Duration::from_millis(200));
        let cfg = WorkerGatewayConfig {
            base_url,
            timeout,
            auth_token,
            retries,
            backoff_base,
        };
        if let Ok(backend) = HttpWorkerBackend::new(cfg) {
            return Arc::new(backend);
        } else {
            warn!("configured WORKER_GATEWAY_URL but failed to init HTTP backend; using stub");
        }
    } else {
        info!("WORKER_GATEWAY_URL not set; using stub worker backend");
    }
    Arc::new(StubWorkerBackend)
}

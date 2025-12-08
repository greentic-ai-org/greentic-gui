mod api;
mod auth;
mod config;
mod fragments;
mod integration;
mod packs;
mod routing;
mod sdk;
mod server;
mod tenant;
mod worker;

use crate::config::AppConfig;
use crate::fragments::{CompositeFragmentRenderer, NoopFragmentInvoker, WasmtimeFragmentInvoker};
use crate::integration::{GreenticTelemetrySink, RealSessionManager};
use crate::packs::FsPackProvider;
use crate::server::AppState;
use crate::worker::{WorkerHost, worker_backend_from_env};
use greentic_distributor_client::{
    HttpDistributorClient, config::DistributorClientConfig, types::DistributorEnvironmentId,
};
use greentic_session::{SessionBackendConfig, create_session_store};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    GreenticTelemetrySink::init();
    let config = AppConfig::from_env()?;
    let pack_provider: Arc<dyn crate::packs::PackProvider> = if let Some(dist) = &config.distributor
    {
        let cfg = DistributorClientConfig {
            base_url: Some(dist.base_url.clone()),
            environment_id: DistributorEnvironmentId::from(dist.environment_id.clone()),
            tenant: greentic_types::TenantCtx::new(
                greentic_types::EnvId::new(&config.env_id)?,
                greentic_types::TenantId::new(&config.default_tenant)?,
            ),
            auth_token: dist.auth_token.clone(),
            extra_headers: None,
            request_timeout: None,
        };
        let client = HttpDistributorClient::new(cfg)?;
        if let Some(packs_json) = &dist.packs_json {
            Arc::new(crate::packs::distributor_provider_from_json(
                client,
                DistributorEnvironmentId::from(dist.environment_id.clone()),
                packs_json,
            )?)
        } else {
            Arc::new(FsPackProvider::new(config.pack_root.clone()))
        }
    } else {
        Arc::new(FsPackProvider::new(config.pack_root.clone()))
    };
    let wit_invoker: Arc<dyn crate::fragments::FragmentInvoker> =
        match WasmtimeFragmentInvoker::new() {
            Ok(inv) => Arc::new(inv),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "failed to init wasmtime fragment invoker; falling back to noop"
                );
                Arc::new(NoopFragmentInvoker)
            }
        };
    let fragment_renderer = Arc::new(CompositeFragmentRenderer::with_wit(wit_invoker));
    let session_backend = if let Ok(redis_url) = std::env::var("REDIS_URL") {
        tracing::info!("using Redis session store");
        SessionBackendConfig::RedisUrl(redis_url)
    } else {
        SessionBackendConfig::InMemory
    };
    let session_store: Arc<dyn greentic_session::store::SessionStore> =
        match create_session_store(session_backend) {
            Ok(store) => Arc::from(store),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "failed to init configured session store; falling back to in-memory"
                );
                Arc::from(
                    create_session_store(SessionBackendConfig::InMemory)
                        .expect("in-memory session store should be available"),
                )
            }
        };
    let session_manager: Arc<dyn crate::integration::SessionManager> =
        Arc::new(RealSessionManager::new(session_store));
    let telemetry: Arc<dyn crate::integration::TelemetrySink> = Arc::new(GreenticTelemetrySink);
    let worker_backend = worker_backend_from_env();
    let worker_host = Arc::new(WorkerHost::new(worker_backend));

    let state = AppState::new(
        config.clone(),
        pack_provider,
        fragment_renderer,
        session_manager,
        telemetry,
        worker_host,
    );

    let addr: SocketAddr = config.bind_addr;
    tracing::info!(%addr, "starting greentic-gui server");
    server::run(addr, state).await?;
    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
}

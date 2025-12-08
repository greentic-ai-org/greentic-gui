use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Runtime configuration for the GUI server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub bind_addr: SocketAddr,
    pub pack_root: PathBuf,
    pub default_tenant: String,
    pub enable_cors: bool,
    pub pack_cache_ttl: Duration,
    pub session_ttl: Duration,
    pub tenant_map: TenantMap,
    pub env_id: String,
    pub default_team: String,
    pub distributor: Option<DistributorConfig>,
    pub oauth_broker_url: Option<String>,
    pub oauth_issuer: Option<String>,
    pub oauth_audience: Option<String>,
    pub oauth_jwks_url: Option<String>,
    #[serde(default)]
    pub oauth_required_scopes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TenantMap(pub std::collections::HashMap<String, String>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributorConfig {
    pub base_url: String,
    pub environment_id: String,
    pub auth_token: Option<String>,
    /// JSON string mapping pack kind to {pack_id, component_id, version}
    pub packs_json: Option<String>,
}

impl AppConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_addr: SocketAddr = std::env::var("BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
            .parse()
            .context("failed to parse BIND_ADDR")?;

        let pack_root =
            PathBuf::from(std::env::var("PACK_ROOT").unwrap_or_else(|_| "packs".to_string()));

        let default_tenant =
            std::env::var("DEFAULT_TENANT").unwrap_or_else(|_| "tenant-default".to_string());

        let enable_cors = std::env::var("ENABLE_CORS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let env_id = std::env::var("GREENTIC_ENV").unwrap_or_else(|_| "dev".to_string());
        let default_team = std::env::var("GREENTIC_TEAM").unwrap_or_else(|_| "gui".to_string());
        let oauth_broker_url = std::env::var("OAUTH_BROKER_URL").ok();
        let oauth_issuer = std::env::var("OAUTH_ISSUER").ok();
        let oauth_audience = std::env::var("OAUTH_AUDIENCE").ok();
        let oauth_jwks_url = std::env::var("OAUTH_JWKS_URL").ok();
        let oauth_required_scopes = std::env::var("OAUTH_REQUIRED_SCOPES")
            .ok()
            .map(|s| {
                s.split(',')
                    .filter(|v| !v.is_empty())
                    .map(|v| v.trim().to_string())
                    .collect()
            })
            .unwrap_or_default();

        let distributor =
            std::env::var("GREENTIC_DISTRIBUTOR_URL")
                .ok()
                .map(|base| DistributorConfig {
                    base_url: base,
                    environment_id: std::env::var("GREENTIC_DISTRIBUTOR_ENV")
                        .unwrap_or_else(|_| env_id.clone()),
                    auth_token: std::env::var("GREENTIC_DISTRIBUTOR_TOKEN").ok(),
                    packs_json: std::env::var("GREENTIC_DISTRIBUTOR_PACKS").ok(),
                });

        let pack_cache_ttl = std::env::var("PACK_CACHE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(0));
        let session_ttl = std::env::var("SESSION_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(0));

        let tenant_map = std::env::var("TENANT_MAP_JSON")
            .ok()
            .and_then(|v| {
                serde_json::from_str::<std::collections::HashMap<String, String>>(&v).ok()
            })
            .map(TenantMap)
            .unwrap_or_default();

        Ok(Self {
            bind_addr,
            pack_root,
            default_tenant,
            enable_cors,
            pack_cache_ttl,
            session_ttl,
            tenant_map,
            env_id,
            default_team,
            distributor,
            oauth_broker_url,
            oauth_issuer,
            oauth_audience,
            oauth_jwks_url,
            oauth_required_scopes,
        })
    }

    pub fn tenant_for_domain<'a>(&'a self, domain: &'a str) -> &'a str {
        self.tenant_map
            .0
            .get(domain)
            .map(|s| s.as_str())
            .unwrap_or(&self.default_tenant)
    }
}

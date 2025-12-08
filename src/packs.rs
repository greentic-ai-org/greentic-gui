use anyhow::{Context, anyhow};
use async_trait::async_trait;
use greentic_distributor_client::{
    DistributorClient, HttpDistributorClient,
    types::{ArtifactLocation, DistributorEnvironmentId, ResolveComponentRequest},
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::fs as tokio_fs;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tracing::warn;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
#[allow(clippy::enum_variant_names)]
pub enum PackKind {
    GuiLayout,
    GuiAuth,
    GuiFeature,
    GuiSkin,
    GuiTelemetry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributorPackRef {
    pub pack_id: String,
    pub component_id: String,
    pub version: String,
}

pub struct DistributorPackProvider {
    client: HttpDistributorClient,
    env_id: DistributorEnvironmentId,
    /// mapping from kind -> pack ref
    packs: HashMap<PackKind, DistributorPackRef>,
    cache: tokio::sync::Mutex<HashMap<String, PathBuf>>,
}

impl DistributorPackProvider {
    pub fn new(
        client: HttpDistributorClient,
        env_id: DistributorEnvironmentId,
        packs: HashMap<PackKind, DistributorPackRef>,
    ) -> Self {
        Self {
            client,
            env_id,
            packs,
            cache: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    async fn resolve(&self, tenant: &str, kind: PackKind) -> anyhow::Result<Option<PathBuf>> {
        let Some(pack_ref) = self.packs.get(&kind) else {
            return Ok(None);
        };
        let cache_key = format!("{}::{:?}", tenant, kind);
        if let Some(path) = self.cache.lock().await.get(&cache_key).cloned() {
            return Ok(Some(path));
        }

        let tenant_ctx = greentic_types::TenantCtx::new(
            greentic_types::EnvId::new(self.env_id.as_str())?,
            greentic_types::TenantId::new(tenant)?,
        );

        let req = ResolveComponentRequest {
            tenant: tenant_ctx,
            environment_id: self.env_id.clone(),
            pack_id: pack_ref.pack_id.clone(),
            component_id: pack_ref.component_id.clone(),
            version: pack_ref.version.clone(),
            extra: serde_json::Value::Null,
        };

        let resp = self.client.resolve_component(req).await?;
        let path = match resp.artifact {
            ArtifactLocation::FilePath { path } => PathBuf::from(path),
            ArtifactLocation::OciReference { reference } => self.download_oci(&reference).await?,
            ArtifactLocation::DistributorInternal { handle } => {
                self.materialize_internal(&handle).await?
            }
        };
        self.cache.lock().await.insert(cache_key, path.clone());
        Ok(Some(path))
    }

    async fn load_manifest_from_path(&self, root: PathBuf) -> anyhow::Result<serde_json::Value> {
        let manifest_path = root.join("gui").join("manifest.json");
        let mut file = tokio_fs::File::open(&manifest_path)
            .await
            .with_context(|| format!("opening manifest {:?}", manifest_path))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).await?;
        let json: serde_json::Value = serde_json::from_slice(&buf)?;
        Ok(json)
    }

    async fn load_pack(&self, tenant: &str, kind: PackKind) -> anyhow::Result<Option<GuiPack>> {
        let Some(root) = self.resolve(tenant, kind.clone()).await? else {
            return Ok(None);
        };
        let manifest_json = self.load_manifest_from_path(root.clone()).await?;
        let gui_pack = match manifest_json.get("kind").and_then(|v| v.as_str()) {
            Some("gui-layout") if kind == PackKind::GuiLayout => {
                let manifest: LayoutManifest = serde_json::from_value(manifest_json)?;
                Some(GuiPack::Layout { manifest, root })
            }
            Some("gui-auth") if kind == PackKind::GuiAuth => {
                let manifest: AuthManifest = serde_json::from_value(manifest_json)?;
                Some(GuiPack::Auth { manifest, root })
            }
            Some("gui-feature") if kind == PackKind::GuiFeature => {
                let manifest: FeatureManifest = serde_json::from_value(manifest_json)?;
                Some(GuiPack::Feature { manifest, root })
            }
            Some("gui-skin") if kind == PackKind::GuiSkin => Some(GuiPack::Skin {
                manifest: manifest_json,
                root,
            }),
            Some("gui-telemetry") if kind == PackKind::GuiTelemetry => Some(GuiPack::Telemetry {
                manifest: manifest_json,
                root,
            }),
            _ => None,
        };
        Ok(gui_pack)
    }

    async fn materialize_internal(&self, handle: &str) -> anyhow::Result<PathBuf> {
        // Temporary workaround: treat handle as a local path if it exists; otherwise error.
        let path = PathBuf::from(handle);
        if path.exists() {
            return Ok(path);
        }
        Err(anyhow!(
            "unsupported distributor internal artifact handle (not a local path): {handle}"
        ))
    }

    async fn download_oci(&self, reference: &str) -> anyhow::Result<PathBuf> {
        let tmp_dir = std::env::temp_dir().join(format!("greentic_gui_{}", Uuid::new_v4()));
        tokio_fs::create_dir_all(&tmp_dir).await?;
        let archive_path = tmp_dir.join("artifact.tar");
        let client = reqwest::Client::builder()
            .user_agent("greentic-gui/0.1")
            .build()?;
        let mut req = client.get(reference);
        let bearer = std::env::var("GREENTIC_OCI_BEARER").ok();
        let user = std::env::var("GREENTIC_OCI_USERNAME").ok();
        let pass = std::env::var("GREENTIC_OCI_PASSWORD").ok();
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        } else if let (Some(user), Some(pass)) = (user.clone(), pass.clone()) {
            req = req.basic_auth(user, Some(pass));
        } else if user.is_some() || pass.is_some() {
            warn!(
                "GREENTIC_OCI_USERNAME or GREENTIC_OCI_PASSWORD set without both values; continuing unauthenticated"
            );
        }
        let mut resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!(
                "failed to download OCI reference {}: status {}",
                reference,
                resp.status()
            ));
        }
        let mut file = tokio_fs::File::create(&archive_path).await?;
        while let Some(chunk) = resp.chunk().await? {
            file.write_all(&chunk).await?;
        }
        // naive extraction: assume tar.gz or tar; attempt both
        if let Err(err) = tokio::task::spawn_blocking({
            let archive_path = archive_path.clone();
            let tmp_dir = tmp_dir.clone();
            move || -> anyhow::Result<()> {
                let file = std::fs::File::open(&archive_path)?;
                let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
                archive.unpack(&tmp_dir)?;
                Ok(())
            }
        })
        .await?
        {
            warn!(?err, "tar.gz extraction failed; trying plain tar");
            let _ = tokio::task::spawn_blocking({
                let archive_path = archive_path.clone();
                let tmp_dir = tmp_dir.clone();
                move || -> anyhow::Result<()> {
                    let file = std::fs::File::open(&archive_path)?;
                    let mut archive = tar::Archive::new(file);
                    archive.unpack(&tmp_dir)?;
                    Ok(())
                }
            })
            .await?;
        }
        Ok(tmp_dir)
    }

    pub async fn reset_cache(&self) {
        let mut cache = self.cache.lock().await;
        cache.clear();
        tracing::info!("pack cache cleared");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutManifest {
    pub kind: String,
    pub layout: LayoutConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutConfig {
    pub slots: Vec<String>,
    pub entrypoint_html: String,
    pub spa: bool,
    pub slot_selectors: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthManifest {
    pub kind: String,
    pub routes: Vec<AuthRoute>,
    pub oauth: serde_json::Value,
    pub ui_bindings: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRoute {
    pub path: String,
    #[serde(default)]
    pub public: bool,
    pub html: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureManifest {
    pub kind: String,
    pub routes: Vec<FeatureRoute>,
    #[serde(default)]
    pub digital_workers: Vec<DigitalWorker>,
    #[serde(default)]
    pub fragments: Vec<FragmentBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureRoute {
    pub path: String,
    #[serde(default)]
    pub authenticated: bool,
    pub html: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigitalWorker {
    pub id: String,
    pub worker_id: String,
    pub attach: WorkerAttach,
    pub routes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerAttach {
    pub mode: String,
    pub selector: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FragmentBinding {
    pub id: String,
    pub selector: String,
    #[serde(rename = "component_world")]
    pub component_world: String,
    #[serde(rename = "component_name")]
    pub component_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum GuiPack {
    #[serde(rename = "gui-layout")]
    Layout {
        manifest: LayoutManifest,
        root: PathBuf,
    },
    #[serde(rename = "gui-auth")]
    Auth {
        manifest: AuthManifest,
        root: PathBuf,
    },
    #[serde(rename = "gui-feature")]
    Feature {
        manifest: FeatureManifest,
        root: PathBuf,
    },
    #[serde(rename = "gui-skin")]
    Skin {
        manifest: serde_json::Value,
        root: PathBuf,
    },
    #[serde(rename = "gui-telemetry")]
    Telemetry {
        manifest: serde_json::Value,
        root: PathBuf,
    },
}

impl GuiPack {
    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        match self {
            GuiPack::Layout { root, .. }
            | GuiPack::Auth { root, .. }
            | GuiPack::Feature { root, .. }
            | GuiPack::Skin { root, .. }
            | GuiPack::Telemetry { root, .. } => root.as_path(),
        }
    }

    #[allow(dead_code)]
    pub fn assets_root(&self) -> PathBuf {
        self.root().join("gui").join("assets")
    }
}

#[async_trait]
pub trait PackProvider: Send + Sync {
    async fn load_layout(&self, tenant: &str) -> anyhow::Result<GuiPack>;
    async fn load_auth(&self, tenant: &str) -> anyhow::Result<Option<GuiPack>>;
    async fn load_skin(&self, tenant: &str) -> anyhow::Result<Option<GuiPack>>;
    async fn load_telemetry(&self, tenant: &str) -> anyhow::Result<Option<GuiPack>>;
    async fn load_features(&self, tenant: &str) -> anyhow::Result<Vec<GuiPack>>;
    async fn clear_cache(&self);
}

/// File-system backed pack provider for development and tests.
pub struct FsPackProvider {
    root: PathBuf,
}

impl FsPackProvider {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    async fn load_manifest(&self, path: &Path) -> anyhow::Result<serde_json::Value> {
        let manifest_path = path.join("gui").join("manifest.json");
        let mut file = tokio_fs::File::open(&manifest_path)
            .await
            .with_context(|| format!("opening manifest {:?}", manifest_path))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).await?;
        let json: serde_json::Value = serde_json::from_slice(&buf)?;
        Ok(json)
    }

    fn tenant_pack_root(&self, tenant: &str, pack_name: &str) -> PathBuf {
        self.root.join(tenant).join(pack_name)
    }

    fn discover_packs(&self, tenant: &str) -> anyhow::Result<Vec<String>> {
        let root = self.root.join(tenant);
        if !root.exists() {
            return Ok(vec![]);
        }
        let entries = fs::read_dir(root)?;
        let mut packs = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.path().is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                packs.push(name.to_string());
            }
        }
        Ok(packs)
    }
}

#[async_trait]
impl PackProvider for FsPackProvider {
    async fn load_layout(&self, tenant: &str) -> anyhow::Result<GuiPack> {
        let candidates = self.discover_packs(tenant)?;
        for name in candidates {
            let pack_root = self.tenant_pack_root(tenant, &name);
            let manifest_json = self.load_manifest(&pack_root).await?;
            if manifest_json.get("kind").and_then(|v| v.as_str()) == Some("gui-layout") {
                let manifest: LayoutManifest = serde_json::from_value(manifest_json.clone())
                    .context("parse layout manifest")?;
                return Ok(GuiPack::Layout {
                    manifest,
                    root: pack_root,
                });
            }
        }
        Err(anyhow!("no layout pack found for tenant {}", tenant))
    }

    async fn load_auth(&self, tenant: &str) -> anyhow::Result<Option<GuiPack>> {
        let candidates = self.discover_packs(tenant)?;
        for name in candidates {
            let pack_root = self.tenant_pack_root(tenant, &name);
            let manifest_json = self.load_manifest(&pack_root).await?;
            if manifest_json.get("kind").and_then(|v| v.as_str()) == Some("gui-auth") {
                let manifest: AuthManifest =
                    serde_json::from_value(manifest_json.clone()).context("parse auth manifest")?;
                return Ok(Some(GuiPack::Auth {
                    manifest,
                    root: pack_root,
                }));
            }
        }
        Ok(None)
    }

    async fn load_skin(&self, tenant: &str) -> anyhow::Result<Option<GuiPack>> {
        let candidates = self.discover_packs(tenant)?;
        for name in candidates {
            let pack_root = self.tenant_pack_root(tenant, &name);
            let manifest_json = self.load_manifest(&pack_root).await?;
            if manifest_json.get("kind").and_then(|v| v.as_str()) == Some("gui-skin") {
                return Ok(Some(GuiPack::Skin {
                    manifest: manifest_json,
                    root: pack_root,
                }));
            }
        }
        Ok(None)
    }

    async fn load_telemetry(&self, tenant: &str) -> anyhow::Result<Option<GuiPack>> {
        let candidates = self.discover_packs(tenant)?;
        for name in candidates {
            let pack_root = self.tenant_pack_root(tenant, &name);
            let manifest_json = self.load_manifest(&pack_root).await?;
            if manifest_json.get("kind").and_then(|v| v.as_str()) == Some("gui-telemetry") {
                return Ok(Some(GuiPack::Telemetry {
                    manifest: manifest_json,
                    root: pack_root,
                }));
            }
        }
        Ok(None)
    }

    async fn load_features(&self, tenant: &str) -> anyhow::Result<Vec<GuiPack>> {
        let candidates = self.discover_packs(tenant)?;
        let mut features = Vec::new();
        for name in candidates {
            let pack_root = self.tenant_pack_root(tenant, &name);
            let manifest_json = self.load_manifest(&pack_root).await?;
            if manifest_json.get("kind").and_then(|v| v.as_str()) == Some("gui-feature") {
                let manifest: FeatureManifest = serde_json::from_value(manifest_json.clone())
                    .context("parse feature manifest")?;
                features.push(GuiPack::Feature {
                    manifest,
                    root: pack_root,
                });
            }
        }
        Ok(features)
    }

    async fn clear_cache(&self) {}
}

#[async_trait]
impl PackProvider for DistributorPackProvider {
    async fn load_layout(&self, tenant: &str) -> anyhow::Result<GuiPack> {
        let Some(pack) = self.load_pack(tenant, PackKind::GuiLayout).await? else {
            return Err(anyhow!("no layout pack configured for tenant {}", tenant));
        };
        Ok(pack)
    }

    async fn load_auth(&self, tenant: &str) -> anyhow::Result<Option<GuiPack>> {
        self.load_pack(tenant, PackKind::GuiAuth).await
    }

    async fn load_skin(&self, tenant: &str) -> anyhow::Result<Option<GuiPack>> {
        self.load_pack(tenant, PackKind::GuiSkin).await
    }

    async fn load_telemetry(&self, tenant: &str) -> anyhow::Result<Option<GuiPack>> {
        self.load_pack(tenant, PackKind::GuiTelemetry).await
    }

    async fn load_features(&self, tenant: &str) -> anyhow::Result<Vec<GuiPack>> {
        match self.load_pack(tenant, PackKind::GuiFeature).await? {
            Some(pack) => Ok(vec![pack]),
            None => Ok(vec![]),
        }
    }

    async fn clear_cache(&self) {
        self.reset_cache().await;
    }
}

pub fn normalize_route(path: &str) -> String {
    let re = Regex::new(r"/+").unwrap();
    let normalized = re.replace_all(path, "/");
    let mut s = normalized.trim().to_string();
    if !s.starts_with('/') {
        s = format!("/{}", s);
    }
    s
}

/// Build a distributor-backed pack provider using config JSON mapping.
pub fn distributor_provider_from_json(
    client: HttpDistributorClient,
    env_id: DistributorEnvironmentId,
    packs_json: &str,
) -> anyhow::Result<DistributorPackProvider> {
    let map: HashMap<String, DistributorPackRef> = serde_json::from_str(packs_json)?;
    let mut packs = HashMap::new();
    for (k, v) in map {
        let kind = match k.as_str() {
            "layout" | "gui-layout" => PackKind::GuiLayout,
            "auth" | "gui-auth" => PackKind::GuiAuth,
            "feature" | "gui-feature" => PackKind::GuiFeature,
            "skin" | "gui-skin" => PackKind::GuiSkin,
            "telemetry" | "gui-telemetry" => PackKind::GuiTelemetry,
            _ => continue,
        };
        packs.insert(kind, v);
    }
    Ok(DistributorPackProvider::new(client, env_id, packs))
}

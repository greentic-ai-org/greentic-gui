use crate::packs::{
    AuthManifest, FeatureManifest, GuiPack, LayoutManifest, PackProvider, normalize_route,
};
use greentic_types::SecretRequirement;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantGuiConfig {
    pub tenant_did: String,
    pub domain: String,
    pub layout: LayoutPack,
    pub auth: Option<AuthPack>,
    pub skin: Option<PackLocation>,
    pub telemetry: Option<PackLocation>,
    pub features: Vec<FeaturePack>,
    pub secret_requirements: Vec<SecretRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutPack {
    pub manifest: LayoutManifest,
    pub location: PackLocation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_requirements: Vec<SecretRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthPack {
    pub manifest: AuthManifest,
    pub location: PackLocation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_requirements: Vec<SecretRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturePack {
    pub manifest: FeatureManifest,
    pub location: PackLocation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_requirements: Vec<SecretRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackLocation {
    pub root: std::path::PathBuf,
    pub assets: std::path::PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_requirements: Vec<SecretRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_hint: Option<String>,
}

impl TenantGuiConfig {
    pub async fn load(
        tenant: &str,
        domain: &str,
        pack_provider: Arc<dyn PackProvider>,
    ) -> anyhow::Result<Self> {
        let layout_pack = match pack_provider.load_layout(tenant).await? {
            GuiPack::Layout {
                manifest,
                root,
                secret_requirements,
                pack_hint,
            } => LayoutPack {
                manifest: manifest.clone(),
                location: PackLocation {
                    assets: root.join("gui").join("assets"),
                    root,
                    secret_requirements: secret_requirements.clone(),
                    pack_hint,
                },
                secret_requirements,
            },
            _ => unreachable!(),
        };

        let auth = match pack_provider.load_auth(tenant).await? {
            Some(GuiPack::Auth {
                manifest,
                root,
                pack_hint,
                secret_requirements,
                ..
            }) => Some(AuthPack {
                manifest,
                location: PackLocation {
                    assets: root.join("gui").join("assets"),
                    root,
                    secret_requirements: secret_requirements.clone(),
                    pack_hint,
                },
                secret_requirements,
            }),
            _ => None,
        };

        let skin = match pack_provider.load_skin(tenant).await? {
            Some(GuiPack::Skin {
                root,
                pack_hint,
                secret_requirements,
                ..
            }) => Some(PackLocation {
                assets: root.join("gui").join("assets"),
                root,
                secret_requirements,
                pack_hint,
            }),
            _ => None,
        };

        let telemetry = match pack_provider.load_telemetry(tenant).await? {
            Some(GuiPack::Telemetry {
                root,
                pack_hint,
                secret_requirements,
                ..
            }) => Some(PackLocation {
                assets: root.join("gui").join("assets"),
                root,
                secret_requirements,
                pack_hint,
            }),
            _ => None,
        };

        let feature_packs = pack_provider
            .load_features(tenant)
            .await?
            .into_iter()
            .filter_map(|pack| match pack {
                GuiPack::Feature {
                    manifest,
                    root,
                    pack_hint,
                    secret_requirements,
                    ..
                } => Some(FeaturePack {
                    manifest,
                    location: PackLocation {
                        assets: root.join("gui").join("assets"),
                        root,
                        secret_requirements: secret_requirements.clone(),
                        pack_hint,
                    },
                    secret_requirements,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();

        let mut secret_requirements = layout_pack.secret_requirements.clone();
        if let Some(auth) = &auth {
            secret_requirements.extend(auth.secret_requirements.clone());
        }
        if let Some(skin) = &skin {
            secret_requirements.extend(skin.secret_requirements.clone());
        }
        if let Some(telemetry) = &telemetry {
            secret_requirements.extend(telemetry.secret_requirements.clone());
        }
        for feature in &feature_packs {
            secret_requirements.extend(feature.secret_requirements.clone());
        }
        secret_requirements = dedup_requirements(secret_requirements);

        Ok(Self {
            tenant_did: tenant.to_string(),
            domain: domain.to_string(),
            layout: layout_pack,
            auth,
            skin,
            telemetry,
            features: feature_packs,
            secret_requirements,
        })
    }

    pub fn resolve_route(&self, path: &str) -> Option<ResolvedRoute> {
        let path = normalize_route(path);
        for feature in &self.features {
            for route in &feature.manifest.routes {
                if path_matches(&path, &route.path) {
                    let fragments = feature
                        .manifest
                        .fragments
                        .iter()
                        .cloned()
                        .map(|binding| FragmentTarget {
                            binding,
                            assets_root: feature.location.assets.clone(),
                        })
                        .collect();
                    return Some(ResolvedRoute {
                        source: RouteSource::Feature(feature.clone()),
                        html_path: feature.location.assets.join(&route.html),
                        authenticated: route.authenticated,
                        fragments,
                    });
                }
            }
        }

        if let Some(auth) = &self.auth {
            for route in &auth.manifest.routes {
                if path_matches(&path, &route.path) {
                    return Some(ResolvedRoute {
                        source: RouteSource::Auth(auth.clone()),
                        html_path: auth.location.assets.join(&route.html),
                        authenticated: !route.public,
                        fragments: vec![],
                    });
                }
            }
        }

        // fallback to layout entrypoint
        Some(ResolvedRoute {
            source: RouteSource::Layout(self.layout.clone()),
            html_path: self
                .layout
                .location
                .assets
                .join(&self.layout.manifest.layout.entrypoint_html),
            authenticated: false,
            fragments: vec![],
        })
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum RouteSource {
    Layout(LayoutPack),
    Auth(AuthPack),
    Feature(FeaturePack),
}

#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    #[allow(dead_code)]
    pub source: RouteSource,
    pub html_path: std::path::PathBuf,
    pub authenticated: bool,
    pub fragments: Vec<FragmentTarget>,
}

#[derive(Debug, Clone)]
pub struct FragmentTarget {
    pub binding: crate::packs::FragmentBinding,
    pub assets_root: std::path::PathBuf,
}

fn path_matches(path: &str, pattern: &str) -> bool {
    if pattern.ends_with("/*") {
        let base = pattern.trim_end_matches('*');
        return path.starts_with(base.trim_end_matches('/'));
    }
    normalize_route(pattern) == path
}

fn dedup_requirements(requirements: Vec<SecretRequirement>) -> Vec<SecretRequirement> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for req in requirements {
        let key = requirement_key(&req);
        if seen.insert(key) {
            out.push(req);
        }
    }
    out
}

fn requirement_key(req: &SecretRequirement) -> String {
    let scope = req
        .scope
        .as_ref()
        .map(|s| {
            format!(
                "{}/{}/{}",
                s.env,
                s.tenant,
                s.team.as_deref().unwrap_or("_")
            )
        })
        .unwrap_or_else(|| "_/_/_".to_string());
    format!("{}::{}", scope, req.key.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packs::{FeatureRoute, LayoutConfig};
    use greentic_types::{SecretKey, SecretRequirement, SecretScope};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn sample_config() -> TenantGuiConfig {
        TenantGuiConfig {
            tenant_did: "tenant".into(),
            domain: "example.com".into(),
            layout: LayoutPack {
                manifest: LayoutManifest {
                    kind: "gui-layout".into(),
                    layout: LayoutConfig {
                        slots: vec![
                            "header".into(),
                            "menu".into(),
                            "main".into(),
                            "footer".into(),
                        ],
                        entrypoint_html: "index.html".into(),
                        spa: true,
                        slot_selectors: HashMap::new(),
                    },
                },
                location: PackLocation {
                    root: PathBuf::from("/tmp/layout"),
                    assets: PathBuf::from("/tmp/layout/gui/assets"),
                    secret_requirements: vec![],
                    pack_hint: None,
                },
                secret_requirements: vec![],
            },
            auth: None,
            skin: None,
            telemetry: None,
            features: vec![FeaturePack {
                manifest: FeatureManifest {
                    kind: "gui-feature".into(),
                    routes: vec![FeatureRoute {
                        path: "/invoices".into(),
                        authenticated: true,
                        html: "invoices.html".into(),
                    }],
                    digital_workers: vec![],
                    fragments: vec![],
                },
                location: PackLocation {
                    root: PathBuf::from("/tmp/feature"),
                    assets: PathBuf::from("/tmp/feature/gui/assets"),
                    secret_requirements: vec![],
                    pack_hint: None,
                },
                secret_requirements: vec![],
            }],
            secret_requirements: vec![],
        }
    }

    #[test]
    fn matches_feature_route() {
        let cfg = sample_config();
        let resolved = cfg.resolve_route("/invoices").expect("route");
        assert!(resolved.authenticated);
        assert!(
            resolved
                .html_path
                .to_string_lossy()
                .ends_with("invoices.html")
        );
    }

    #[test]
    fn dedups_secret_requirements() {
        let mut req1 = SecretRequirement::default();
        req1.key = SecretKey::new("db/password").unwrap();
        req1.scope = Some(SecretScope {
            env: "dev".into(),
            tenant: "tenant".into(),
            team: None,
        });

        let mut req2 = req1.clone();
        req2.description = Some("duplicate with description".into());

        let deduped = super::dedup_requirements(vec![req1.clone(), req2]);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].key.as_str(), "db/password");
    }
}

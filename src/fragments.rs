use crate::integration::SessionInfo;
use crate::packs::FragmentBinding;
use crate::tenant::FragmentTarget;
use async_trait::async_trait;
use greentic_interfaces_guest::gui_fragment as api;
use greentic_interfaces_wasmtime::gui_gui_fragment_v1_0::{
    Component as GuiFragmentComponent, GuiFragment,
    exports::greentic::gui::fragment_api::FragmentContext as WasmtimeFragmentContext,
};
use kuchiki::NodeRef;
use kuchiki::traits::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, error, warn};
use wasmtime::{Engine, Store, component::Linker};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FragmentContext {
    pub tenant_ctx: String,
    pub user_ctx: String,
    pub route: String,
    pub session_id: String,
}

#[derive(Debug, Error)]
pub enum FragmentError {
    #[allow(dead_code)]
    #[error("renderer error: {0}")]
    Renderer(String),
    #[error("html manipulation failed: {0}")]
    Html(String),
    #[error("missing secrets: {0}")]
    MissingSecrets(String),
}

#[async_trait]
pub trait FragmentRenderer: Send + Sync {
    async fn render_fragment(
        &self,
        binding: &FragmentBinding,
        assets_root: &Path,
        ctx: FragmentContext,
    ) -> Result<Option<String>, FragmentError>;
}

/// Simple renderer that looks for `fragments/{id}.html` under the pack assets root.
pub struct FileFragmentRenderer;

#[async_trait]
impl FragmentRenderer for FileFragmentRenderer {
    async fn render_fragment(
        &self,
        binding: &FragmentBinding,
        assets_root: &Path,
        _ctx: FragmentContext,
    ) -> Result<Option<String>, FragmentError> {
        let path = assets_root
            .join("fragments")
            .join(format!("{}.html", binding.id));
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => Ok(Some(content)),
            Err(err) => {
                warn!(?path, ?err, "fragment file not found; falling back");
                Ok(None)
            }
        }
    }
}

#[async_trait]
pub trait FragmentInvoker: Send + Sync {
    async fn render(
        &self,
        component_world: &str,
        component_name: &str,
        fragment_id: &str,
        assets_root: &Path,
        ctx: api::FragmentContext,
    ) -> Result<String, String>;
}

/// Renderer that invokes WIT gui-fragment components via a pluggable invoker.
pub struct WitFragmentRenderer {
    invoker: Arc<dyn FragmentInvoker>,
}

impl WitFragmentRenderer {
    pub fn new(invoker: Arc<dyn FragmentInvoker>) -> Self {
        Self { invoker }
    }
}

#[async_trait]
impl FragmentRenderer for WitFragmentRenderer {
    async fn render_fragment(
        &self,
        binding: &FragmentBinding,
        _assets_root: &Path,
        ctx: FragmentContext,
    ) -> Result<Option<String>, FragmentError> {
        let api_ctx = api::FragmentContext {
            tenant_ctx: ctx.tenant_ctx,
            user_ctx: ctx.user_ctx,
            route: ctx.route,
            session_id: ctx.session_id,
        };
        match self
            .invoker
            .render(
                &binding.component_world,
                &binding.component_name,
                &binding.id,
                _assets_root,
                api_ctx,
            )
            .await
        {
            Ok(html) => Ok(Some(html)),
            Err(err) => {
                if err.contains("missing_secrets") {
                    return Err(FragmentError::MissingSecrets(err));
                }
                warn!(id = %binding.id, %err, "wit fragment render failed");
                Ok(None)
            }
        }
    }
}

/// Composite renderer that tries WIT components first, then falls back to file fragments.
pub struct CompositeFragmentRenderer {
    wit: Option<WitFragmentRenderer>,
    file: FileFragmentRenderer,
}

impl CompositeFragmentRenderer {
    pub fn with_wit(invoker: Arc<dyn FragmentInvoker>) -> Self {
        Self {
            wit: Some(WitFragmentRenderer::new(invoker)),
            file: FileFragmentRenderer,
        }
    }

    #[allow(dead_code)]
    pub fn file_only() -> Self {
        Self {
            wit: None,
            file: FileFragmentRenderer,
        }
    }
}

#[async_trait]
impl FragmentRenderer for CompositeFragmentRenderer {
    async fn render_fragment(
        &self,
        binding: &FragmentBinding,
        assets_root: &Path,
        ctx: FragmentContext,
    ) -> Result<Option<String>, FragmentError> {
        if let Some(wit) = &self.wit
            && let Some(html) = wit
                .render_fragment(binding, assets_root, ctx.clone())
                .await?
        {
            return Ok(Some(html));
        }
        self.file.render_fragment(binding, assets_root, ctx).await
    }
}

/// In-memory invoker placeholder; returns Err to trigger fallback.
pub struct NoopFragmentInvoker;

#[async_trait]
impl FragmentInvoker for NoopFragmentInvoker {
    async fn render(
        &self,
        _component_world: &str,
        _component_name: &str,
        _fragment_id: &str,
        _assets_root: &Path,
        _ctx: api::FragmentContext,
    ) -> Result<String, String> {
        Err("wit fragment invoker not configured".to_string())
    }
}

/// Wasmtime-based invoker that loads wasm components from `assets_root/fragments/{component_name}.wasm`.
pub struct WasmtimeFragmentInvoker {
    engine: Engine,
    cache: RwLock<HashMap<PathBuf, Arc<wasmtime::component::Component>>>,
}

impl WasmtimeFragmentInvoker {
    pub fn new() -> anyhow::Result<Self> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        let engine = Engine::new(&config)?;
        Ok(Self {
            engine,
            cache: RwLock::new(HashMap::new()),
        })
    }

    async fn instantiate(&self, component_path: &Path) -> anyhow::Result<(GuiFragment, Store<()>)> {
        let component = if let Some(cached) = self.cache.read().await.get(component_path).cloned() {
            cached
        } else {
            let wasm_bytes = tokio::fs::read(component_path).await?;
            let compiled = GuiFragmentComponent::instantiate(&self.engine, &wasm_bytes)?;
            let arc = Arc::new(compiled);
            self.cache
                .write()
                .await
                .insert(component_path.to_path_buf(), arc.clone());
            arc
        };

        let linker = Linker::new(&self.engine);
        let mut store = Store::new(&self.engine, ());
        let bindings = GuiFragment::instantiate(&mut store, &component, &linker)?;
        Ok((bindings, store))
    }
}

#[async_trait]
impl FragmentInvoker for WasmtimeFragmentInvoker {
    async fn render(
        &self,
        _component_world: &str,
        component_name: &str,
        fragment_id: &str,
        assets_root: &Path,
        ctx: api::FragmentContext,
    ) -> Result<String, String> {
        let component_path = assets_root
            .join("fragments")
            .join(format!("{component_name}.wasm"));
        let (bindings, mut store) = self
            .instantiate(&component_path)
            .await
            .map_err(|e| e.to_string())?;

        let ctx_bindgen = WasmtimeFragmentContext {
            tenant_ctx: ctx.tenant_ctx,
            user_ctx: ctx.user_ctx,
            route: ctx.route,
            session_id: ctx.session_id,
        };

        let html = bindings
            .greentic_gui_fragment_api()
            .call_render_fragment(&mut store, fragment_id, &ctx_bindgen)
            .map_err(|e| e.to_string())??;

        Ok(html)
    }
}

pub async fn inject_fragments(
    html: String,
    bindings: &[FragmentTarget],
    session: Option<&SessionInfo>,
    tenant_did: &str,
    route: &str,
    renderer: Arc<dyn FragmentRenderer>,
) -> Result<String, FragmentError> {
    if bindings.is_empty() {
        return Ok(html);
    }

    let mut rendered: Vec<(FragmentBinding, String, PathBuf)> = Vec::new();
    for target in bindings {
        let binding = &target.binding;
        let ctx = FragmentContext {
            tenant_ctx: tenant_did.to_string(),
            user_ctx: session
                .and_then(|s| s.user_id.clone())
                .unwrap_or_else(|| "{}".to_string()),
            route: route.to_string(),
            session_id: session.map(|s| s.session_id.clone()).unwrap_or_default(),
        };
        match renderer
            .render_fragment(binding, &target.assets_root, ctx)
            .await
        {
            Ok(Some(fragment_html)) => {
                rendered.push((binding.clone(), fragment_html, target.assets_root.clone()));
            }
            Ok(None) => {
                debug!(id = %binding.id, "fragment renderer returned None");
            }
            Err(err) => {
                if let FragmentError::MissingSecrets(msg) = &err {
                    warn!(
                        id = %binding.id,
                        selector = %binding.selector,
                        assets = %target.assets_root.display(),
                        "fragment missing secrets: {msg}"
                    );
                    let fallback = format!(
                        "<div class=\"fragment-error\" data-fragment-id=\"{}\">missing secrets for fragment</div>",
                        binding.id
                    );
                    rendered.push((binding.clone(), fallback, target.assets_root.clone()));
                    continue;
                }
                error!(
                    id = %binding.id,
                    selector = %binding.selector,
                    assets = %target.assets_root.display(),
                    ?err,
                    "fragment renderer failed"
                );
                let fallback = format!(
                    "<div class=\"fragment-error\" data-fragment-id=\"{}\">fragment render failed</div>",
                    binding.id
                );
                rendered.push((binding.clone(), fallback, target.assets_root.clone()));
            }
        }
    }

    let mut document = kuchiki::parse_html().one(html);
    for (binding, fragment_html, _assets_root) in rendered {
        if let Err(err) =
            replace_selector_inner_html(&mut document, &binding.selector, &fragment_html)
        {
            warn!(?binding, ?err, "failed to inject fragment html");
        }
    }

    Ok(document.to_string())
}

fn replace_selector_inner_html(
    document: &mut NodeRef,
    selector: &str,
    new_html: &str,
) -> Result<(), FragmentError> {
    let mut nodes = document
        .select(selector)
        .map_err(|e| FragmentError::Html(format!("query selector {selector} failed: {e:?}")))?;
    if let Some(node_data) = nodes.next() {
        let node = node_data.as_node();
        // Clear existing children
        let existing: Vec<_> = node.children().collect();
        for child in existing {
            child.detach();
        }

        // Parse fragment wrapped to ensure valid HTML structure
        let wrapper_html = format!("<div id=\"__greentic_fragment_wrapper\">{new_html}</div>");
        let fragment_doc = kuchiki::parse_html().one(wrapper_html);
        let mut frag_nodes = fragment_doc
            .select("#__greentic_fragment_wrapper")
            .map_err(|e| FragmentError::Html(format!("select wrapper failed: {e:?}")))?;
        if let Some(wrapper) = frag_nodes.next() {
            let children: Vec<_> = wrapper.as_node().children().collect();
            for child in children {
                node.append(child);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct DummyRenderer;

    #[async_trait]
    impl FragmentRenderer for DummyRenderer {
        async fn render_fragment(
            &self,
            _binding: &FragmentBinding,
            _assets_root: &Path,
            _ctx: FragmentContext,
        ) -> Result<Option<String>, FragmentError> {
            Ok(Some("<span class=\"injected\">ok</span>".to_string()))
        }
    }

    #[tokio::test]
    async fn injects_fragment_html() {
        let html = "<html><body><div id=\"target\">old</div></body></html>".to_string();
        let bindings = vec![FragmentTarget {
            binding: FragmentBinding {
                id: "test".into(),
                selector: "#target".into(),
                component_world: "greentic:gui/gui-fragment@1.0.0".into(),
                component_name: "fragment".into(),
            },
            assets_root: PathBuf::from("/tmp"),
        }];
        let rendered = inject_fragments(
            html,
            &bindings,
            None,
            "tenant",
            "/",
            Arc::new(DummyRenderer),
        )
        .await
        .unwrap();
        assert!(rendered.contains("class=\"injected\""));
        assert!(!rendered.contains("old"));
    }
}

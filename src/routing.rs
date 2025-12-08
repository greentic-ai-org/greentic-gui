use crate::integration::{SessionInfo, SessionManager};
use crate::tenant::{AuthPack, ResolvedRoute, TenantGuiConfig};
use anyhow::Context;
use tokio::fs;

#[derive(Debug)]
pub enum RouteDecision {
    Serve(Box<RouteContent>),
    Redirect(String),
    NotFound,
}

#[derive(Debug)]
pub struct RouteContent {
    pub html: String,
    pub fragments: Vec<crate::tenant::FragmentTarget>,
    pub session: Option<SessionInfo>,
}

pub async fn resolve_route(
    tenant_cfg: &TenantGuiConfig,
    path: &str,
    session_token: Option<String>,
    session_manager: &dyn SessionManager,
) -> anyhow::Result<RouteDecision> {
    let resolved = tenant_cfg.resolve_route(path);
    let Some(resolved) = resolved else {
        return Ok(RouteDecision::NotFound);
    };

    let requires_auth = resolved.authenticated;
    let session = session_manager.validate(session_token).await?;
    if requires_auth && session.is_none() {
        let login_target = login_path(tenant_cfg.auth.as_ref());
        return Ok(RouteDecision::Redirect(login_target));
    }

    let html = load_html(&resolved).await?;
    Ok(RouteDecision::Serve(Box::new(RouteContent {
        html,
        fragments: resolved.fragments,
        session,
    })))
}

async fn load_html(resolved: &ResolvedRoute) -> anyhow::Result<String> {
    let contents = fs::read_to_string(&resolved.html_path)
        .await
        .with_context(|| format!("reading html {:?}", resolved.html_path))?;
    Ok(contents)
}

fn login_path(auth: Option<&AuthPack>) -> String {
    if let Some(auth) = auth
        && let Some(route) = auth.manifest.routes.first()
    {
        return route.path.clone();
    }
    "/login".to_string()
}

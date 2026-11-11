//! Pure decision logic (no I/O besides the frontend and grant store).

use std::collections::BTreeSet;
use std::sync::Arc;

use iliada_bundle::{Bundle, Permissions};
use iliada_ipc::{DenyReason, GrantScope};
use iliada_types::{AppId, Capability};
use tokio::sync::Mutex;

use crate::prompt::{PromptAnswer, PromptFrontend, PromptRequest, sanitize_untrusted};
use crate::store::GrantStore;

/// What the broker knows about an app: from its validated bundle, never from the guest.
#[derive(Debug, Clone)]
pub struct AppContext {
    pub id: AppId,
    pub name: String,
    pub permissions: Permissions,
}

impl From<&Bundle> for AppContext {
    fn from(b: &Bundle) -> Self {
        Self { id: b.info.id.clone(), name: b.info.name.clone(), permissions: b.permissions.clone() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionSource {
    StoredGrant,
    SessionGrant,
    User,
    Declaration,
    Policy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow { scope: GrantScope, source: DecisionSource },
    Deny { reason: DenyReason, source: DecisionSource },
}

/// System policy hook (MDM / parental controls / per-tier restrictions).
pub type PolicyFn = dyn Fn(&AppContext, &Capability) -> bool + Send + Sync;

pub struct Engine {
    store: Mutex<GrantStore>,
    frontend: Arc<dyn PromptFrontend>,
    policy: Box<PolicyFn>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").field("frontend", &self.frontend).finish_non_exhaustive()
    }
}

impl Engine {
    pub fn new(store: GrantStore, frontend: Arc<dyn PromptFrontend>) -> Self {
        Self { store: Mutex::new(store), frontend, policy: Box::new(|_, _| true) }
    }

    pub fn with_policy(mut self, policy: impl Fn(&AppContext, &Capability) -> bool + Send + Sync + 'static) -> Self {
        self.policy = Box::new(policy);
        self
    }

    pub fn frontend(&self) -> &Arc<dyn PromptFrontend> {
        &self.frontend
    }

    pub async fn decide(
        &self,
        app: &AppContext,
        session_grants: &BTreeSet<Capability>,
        cap: &Capability,
        app_reason: Option<&str>,
    ) -> anyhow::Result<Decision> {
        let Some(decl) = app.permissions.declaration_for(cap) else {
            return Ok(Decision::Deny { reason: DenyReason::NotDeclared, source: DecisionSource::Declaration });
        };
        if !(self.policy)(app, cap) {
            return Ok(Decision::Deny { reason: DenyReason::PolicyDenied, source: DecisionSource::Policy });
        }
        if self.store.lock().await.has(&app.id, cap) {
            return Ok(Decision::Allow { scope: GrantScope::Always, source: DecisionSource::StoredGrant });
        }
        if session_grants.iter().any(|g| g.covers(cap)) {
            return Ok(Decision::Allow { scope: GrantScope::WhileRunning, source: DecisionSource::SessionGrant });
        }
        let req = PromptRequest {
            app_id: app.id.clone(),
            app_name: sanitize_untrusted(&app.name, 64),
            capability: *cap,
            declared_purpose: decl.shown_purpose().to_owned(),
            app_reason: app_reason.map(|r| sanitize_untrusted(r, 200)),
        };
        Ok(match self.frontend.ask(&req).await {
            PromptAnswer::Allow(scope) => {
                if scope == GrantScope::Always {
                    self.store.lock().await.insert(&app.id, *cap)?;
                }
                Decision::Allow { scope, source: DecisionSource::User }
            }
            PromptAnswer::Deny => Decision::Deny { reason: DenyReason::UserDenied, source: DecisionSource::User },
        })
    }

    pub async fn stored(&self, app: &AppId) -> Vec<Capability> {
        self.store.lock().await.list(app)
    }

    pub async fn revoke(&self, app: &AppId, cap: &Capability) -> anyhow::Result<bool> {
        self.store.lock().await.remove(app, cap)
    }
}

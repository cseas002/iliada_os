//! What the assistant can do: Iliada's own operations, nothing else (no
//! shell, no files). Every tool either only reads, or is an *action*, which
//! in Ask mode runs only after the user approves it. System changes still go
//! through the system service, so they are limited to what the user may do.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use iliada_bundle::decisions::{Decision, Decisions, key as permission_key};
use iliada_store::Store;
use iliada_system::client;
use iliada_system::protocol::Request;
use serde_json::{Value, json};

#[derive(Debug)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
    /// Changes something (needs approval in Ask mode).
    pub acts: bool,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

pub fn all() -> Vec<Tool> {
    let none = || obj(json!({}), &[]);
    let app =
        || obj(json!({ "app": { "type": "string", "description": "App name or id, e.g. \"Firefox\"" } }), &["app"]);
    vec![
        Tool {
            name: "get_status",
            description: "This computer: Iliada version and update state, command-line tools, services (on/off), backup state, and whether the user is an administrator.",
            schema: none(),
            acts: false,
        },
        Tool {
            name: "search_store",
            description: "Apps available in the Iliada Store (name, id, category, summary, whether it uses the internet, whether it is installed). Optional query filters by name or summary.",
            schema: obj(json!({ "query": { "type": "string" } }), &[]),
            acts: false,
        },
        Tool {
            name: "list_installed_apps",
            description: "Apps installed in Applications, with their versions.",
            schema: none(),
            acts: false,
        },
        Tool {
            name: "install_app",
            description: "Install (or update) an app from the Store.",
            schema: app(),
            acts: true,
        },
        Tool {
            name: "remove_app",
            description: "Remove an installed app (its own data in the app's private folder is kept).",
            schema: app(),
            acts: true,
        },
        Tool { name: "open_app", description: "Open an installed app in its sandbox.", schema: app(), acts: true },
        Tool {
            name: "update_apps",
            description: "Update all apps now, and install any on the app list that are missing.",
            schema: none(),
            acts: true,
        },
        Tool {
            name: "add_tools",
            description: "Install command-line tools system-wide (nixpkgs attribute names such as \"ripgrep\", \"python3\"). Rebuilds the system; takes a few minutes.",
            schema: obj(json!({ "packages": { "type": "array", "items": { "type": "string" } } }), &["packages"]),
            acts: true,
        },
        Tool {
            name: "remove_tools",
            description: "Uninstall command-line tools. Rebuilds the system.",
            schema: obj(json!({ "packages": { "type": "array", "items": { "type": "string" } } }), &["packages"]),
            acts: true,
        },
        Tool {
            name: "set_service",
            description: "Turn a system service on or off: \"ssh\" (remote login) or \"printing\". Rebuilds the system.",
            schema: obj(
                json!({ "name": { "type": "string", "enum": ["ssh", "printing"] }, "on": { "type": "boolean" } }),
                &["name", "on"],
            ),
            acts: true,
        },
        Tool {
            name: "check_for_iliada_update",
            description: "Look for a new version of Iliada.",
            schema: none(),
            acts: false,
        },
        Tool {
            name: "update_iliada",
            description: "Install the newest Iliada release (a restart finishes it).",
            schema: none(),
            acts: true,
        },
        Tool {
            name: "back_up_now",
            description: "Back up apps and settings to the user's GitHub backup now (only if backup is set up).",
            schema: none(),
            acts: true,
        },
        Tool {
            name: "list_permissions",
            description: "For each installed app: what it may ask for (folders, camera, microphone) and what the user decided.",
            schema: none(),
            acts: false,
        },
        Tool {
            name: "set_permission",
            description: "Allow or deny an app a permission it declares. permission: \"camera\", \"microphone\", or \"files.documents\" / \"files.downloads\" / \"files.music\" / \"files.pictures\" / \"files.videos\". Applies the next time the app opens.",
            schema: obj(
                json!({ "app": { "type": "string" }, "permission": { "type": "string" }, "allow": { "type": "boolean" } }),
                &["app", "permission", "allow"],
            ),
            acts: true,
        },
    ]
}

/// The API's `tools` array.
pub fn definitions() -> Value {
    Value::Array(
        all()
            .into_iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.schema }))
            .collect(),
    )
}

pub fn acts(name: &str) -> bool {
    all().iter().any(|t| t.name == name && t.acts)
}

fn s<'a>(input: &'a Value, k: &str) -> &'a str {
    input[k].as_str().unwrap_or("")
}

fn list(input: &Value, k: &str) -> Vec<String> {
    input[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// How an action reads in an approval prompt.
pub fn describe(name: &str, input: &Value) -> String {
    match name {
        "install_app" => format!("Install {}", s(input, "app")),
        "remove_app" => format!("Remove {}", s(input, "app")),
        "open_app" => format!("Open {}", s(input, "app")),
        "update_apps" => "Update all apps".into(),
        "add_tools" => format!("Install the command-line tools {}", list(input, "packages").join(", ")),
        "remove_tools" => format!("Uninstall the command-line tools {}", list(input, "packages").join(", ")),
        "set_service" => format!(
            "Turn {} {}",
            match s(input, "name") {
                "ssh" => "the SSH server",
                "printing" => "printing",
                other => other,
            },
            if input["on"].as_bool() == Some(true) { "on" } else { "off" }
        ),
        "update_iliada" => "Update Iliada".into(),
        "back_up_now" => "Back up now".into(),
        "set_permission" => format!(
            "{} {} {}",
            if input["allow"].as_bool() == Some(true) { "Allow" } else { "Deny" },
            s(input, "app"),
            s(input, "permission").replace("files.", "the folder ")
        ),
        other => other.to_owned(),
    }
}

/// Installed bundles (id, name, path, bundle).
fn installed_bundles() -> Vec<iliada_bundle::Bundle> {
    std::fs::read_dir("/Applications")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| iliada_bundle::Bundle::open(e.path()).ok())
        .collect()
}

fn find_installed(app: &str) -> Result<iliada_bundle::Bundle> {
    let needle = app.to_lowercase();
    installed_bundles()
        .into_iter()
        .find(|b| b.info.id.as_str() == app || b.info.name.to_lowercase() == needle)
        .with_context(|| format!("{app:?} is not installed"))
}

async fn system(request: Request) -> Result<Value> {
    client::call(&request, &mut |_| {}).await
}

/// Run one tool; the result is text for the model.
pub async fn run(store: &Store, name: &str, input: &Value) -> Result<String> {
    let out: Value = match name {
        "get_status" => serde_json::to_value(client::status().await?)?,
        "search_store" => {
            let q = s(input, "query").to_lowercase();
            let installed = store.installed();
            let catalog = store.catalog().await?;
            Value::Array(
                catalog
                    .apps
                    .iter()
                    .filter(|a| {
                        q.is_empty() || a.name.to_lowercase().contains(&q) || a.summary.to_lowercase().contains(&q)
                    })
                    .map(|a| {
                        json!({ "name": a.name, "id": a.id, "category": a.category, "summary": a.summary,
                                "uses_internet": a.network, "version": a.version,
                                "installed": installed.get(&a.id).map(|i| i.version.clone()) })
                    })
                    .collect(),
            )
        }
        "list_installed_apps" => Value::Array(
            store.installed().values().map(|i| json!({ "name": i.name, "id": i.id, "version": i.version })).collect(),
        ),
        "install_app" => {
            let app = s(input, "app");
            let catalog = store.catalog().await?;
            let needle = app.to_lowercase();
            let entry = catalog
                .apps
                .iter()
                .find(|a| a.id == app || a.name.to_lowercase() == needle)
                .with_context(|| format!("there is no app called {app:?} in the Store"))?;
            let path = store.install(entry, &|_, _| {}).await?;
            json!({ "installed": entry.name, "path": path })
        }
        "remove_app" => {
            let b = find_installed(s(input, "app"))?;
            store.remove(b.info.id.as_str())?;
            json!({ "removed": b.info.name })
        }
        "open_app" => {
            let b = find_installed(s(input, "app"))?;
            std::process::Command::new("iliada-run")
                .arg(b.layout.root())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .context("starting the app")?;
            json!({ "opened": b.info.name })
        }
        "update_apps" => {
            let r = store.sync().await?;
            json!({ "installed": r.installed, "updated": r.updated,
                    "failed": r.failed.iter().map(|(a, e)| format!("{a}: {e}")).collect::<Vec<_>>() })
        }
        "add_tools" => system(Request::AddTools { packages: list(input, "packages") }).await?,
        "remove_tools" => system(Request::RemoveTools { packages: list(input, "packages") }).await?,
        "set_service" => {
            let on = input["on"].as_bool().context("\"on\" must be true or false")?;
            system(Request::SetService { name: s(input, "name").to_owned(), on }).await?
        }
        "check_for_iliada_update" => system(Request::CheckSystemUpdate).await?,
        "update_iliada" => system(Request::UpdateSystem).await?,
        "back_up_now" => system(Request::BackupNow).await?,
        "list_permissions" => {
            let d = Decisions::default_path().map(|p| Decisions::load(&p)).unwrap_or_default();
            Value::Array(
                installed_bundles()
                    .iter()
                    .map(|b| {
                        let perms: Vec<Value> = b
                            .permissions
                            .requests
                            .iter()
                            .map(|r| {
                                json!({ "permission": permission_key(&r.capability),
                                        "decided": d.get(b.info.id.as_str(), &r.capability).map(|x| format!("{x:?}").to_lowercase()) })
                            })
                            .collect();
                        json!({ "app": b.info.name, "id": b.info.id.as_str(), "permissions": perms })
                    })
                    .collect(),
            )
        }
        "set_permission" => {
            let b = find_installed(s(input, "app"))?;
            let wanted = s(input, "permission");
            let req = b
                .permissions
                .requests
                .iter()
                .find(|r| permission_key(&r.capability) == wanted)
                .with_context(|| format!("{} does not ask for {wanted:?}", b.info.name))?;
            let allow = input["allow"].as_bool().context("\"allow\" must be true or false")?;
            let path: PathBuf = Decisions::default_path().context("HOME is not set")?;
            let mut d = Decisions::load(&path);
            d.set(b.info.id.as_str(), &req.capability, if allow { Decision::Allow } else { Decision::Deny });
            d.save(&path)?;
            json!({ "app": b.info.name, "permission": wanted, "allowed": allow, "applies": "next time the app opens" })
        }
        other => bail!("there is no tool called {other}"),
    };
    Ok(out.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_well_formed_and_actions_are_marked() {
        let defs = definitions();
        let names: Vec<&str> = defs.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "unique names");
        for t in defs.as_array().unwrap() {
            assert_eq!(t["input_schema"]["type"], "object");
        }
        assert!(acts("install_app") && acts("set_service") && !acts("get_status") && !acts("search_store"));
        assert!(!acts("no_such_tool"));
        assert_eq!(describe("set_service", &json!({ "name": "ssh", "on": true })), "Turn the SSH server on");
    }
}

//! What the assistant can do: Iliada's own operations and a few desktop
//! settings; with full access (the user's choice) also commands run as the
//! user. Every tool either only reads, or is an *action*, which in Ask mode
//! runs only after the user approves it. System changes still go through the
//! system service, so they are limited to what the user may do.

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
    /// Only with full access (Settings → Assistant).
    pub full_access: bool,
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
            full_access: false,
        },
        Tool {
            name: "search_store",
            description: "Apps available in the Iliada Store (name, id, category, summary, whether it uses the internet, whether it is installed). Optional query filters by name or summary.",
            schema: obj(json!({ "query": { "type": "string" } }), &[]),
            acts: false,
            full_access: false,
        },
        Tool {
            name: "list_installed_apps",
            description: "Apps installed in Applications, with their versions.",
            schema: none(),
            acts: false,
            full_access: false,
        },
        Tool {
            name: "install_app",
            description: "Install (or update) an app from the Store.",
            schema: app(),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "remove_app",
            description: "Remove an installed app (its own data in the app's private folder is kept).",
            schema: app(),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "open_app",
            description: "Open an installed app in its sandbox.",
            schema: app(),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "update_apps",
            description: "Update all apps now, and install any on the app list that are missing.",
            schema: none(),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "add_tools",
            description: "Install command-line tools system-wide (nixpkgs attribute names such as \"ripgrep\", \"python3\"). Rebuilds the system; takes a few minutes.",
            schema: obj(json!({ "packages": { "type": "array", "items": { "type": "string" } } }), &["packages"]),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "remove_tools",
            description: "Uninstall command-line tools. Rebuilds the system.",
            schema: obj(json!({ "packages": { "type": "array", "items": { "type": "string" } } }), &["packages"]),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "list_software",
            description: "The Store's command-line software (Python, Node.js, ffmpeg, …) and background services (nginx, PostgreSQL, …): id, name, kind, and whether each is installed or on. Any other nixpkgs package can also be installed with add_tools.",
            schema: none(),
            acts: false,
            full_access: false,
        },
        Tool {
            name: "set_service",
            description: "Turn a background service on or off. Rebuilds the system.",
            schema: obj(
                json!({
                    "name": { "type": "string", "enum": iliada_system::settings::SERVICES.iter().map(|(n, _)| *n).collect::<Vec<_>>() },
                    "on": { "type": "boolean" }
                }),
                &["name", "on"],
            ),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "check_for_iliada_update",
            description: "Look for a new version of Iliada.",
            schema: none(),
            acts: false,
            full_access: false,
        },
        Tool {
            name: "update_iliada",
            description: "Install the newest Iliada release (a restart finishes it).",
            schema: none(),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "back_up_now",
            description: "Back up apps and settings to the user's GitHub backup now (only if backup is set up).",
            schema: none(),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "list_permissions",
            description: "For each installed app: what it may ask for (folders, camera, microphone) and what the user decided.",
            schema: none(),
            acts: false,
            full_access: false,
        },
        Tool {
            name: "set_permission",
            description: "Allow or deny an app a permission it declares. permission: \"camera\", \"microphone\", or \"files.documents\" / \"files.downloads\" / \"files.music\" / \"files.pictures\" / \"files.videos\". Applies the next time the app opens.",
            schema: obj(
                json!({ "app": { "type": "string" }, "permission": { "type": "string" }, "allow": { "type": "boolean" } }),
                &["app", "permission", "allow"],
            ),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "set_appearance",
            description: "Switch the desktop between dark and light appearance.",
            schema: obj(json!({ "dark": { "type": "boolean" } }), &["dark"]),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "set_volume",
            description: "Set the sound volume (percent, 0 to 100) and/or mute or unmute it.",
            schema: obj(
                json!({ "percent": { "type": "integer", "minimum": 0, "maximum": 100 }, "muted": { "type": "boolean" } }),
                &[],
            ),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "open_link",
            description: "Open a web address (http or https) in the default browser, or a mailto: link in the mail app.",
            schema: obj(json!({ "url": { "type": "string" } }), &["url"]),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "send_email",
            description: "Send an email from the person's own email account (set up in Settings → Assistant → Email). \
                Plain text. The person always sees the email and approves it before it is sent. Write it as they \
                would, and only send what they asked for.",
            schema: obj(
                json!({
                    "to": { "type": "array", "items": { "type": "string" }, "description": "Email addresses" },
                    "cc": { "type": "array", "items": { "type": "string" } },
                    "subject": { "type": "string" },
                    "body": { "type": "string" }
                }),
                &["to", "subject", "body"],
            ),
            acts: true,
            full_access: false,
        },
        Tool {
            name: "run_command",
            description: "Run a command on this computer as the user, with bash (Iliada is NixOS-based; fish is the \
                user's shell). For anything the other tools can't do: reading or changing the user's files, \
                desktop settings, information about the computer. It cannot do what needs an administrator's \
                password: use the other tools for apps, software, services and updates. Prefer read-only \
                commands to look before changing anything. Output is cut after 10,000 characters.",
            schema: obj(
                json!({
                    "command": { "type": "string" },
                    "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": 600, "description": "Default 120." }
                }),
                &["command"],
            ),
            acts: true,
            full_access: true,
        },
    ]
}

/// The API's `tools` array (with or without the full-access tools).
pub fn definitions(full_access: bool) -> Value {
    Value::Array(
        all()
            .into_iter()
            .filter(|t| full_access || !t.full_access)
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.schema }))
            .collect(),
    )
}

pub fn acts(name: &str) -> bool {
    all().iter().any(|t| t.name == name && t.acts)
}

/// Shown to the person before it runs even in Auto mode: what leaves the
/// computer in their name.
pub fn always_asks(name: &str) -> bool {
    name == "send_email"
}

pub fn needs_full_access(name: &str) -> bool {
    all().iter().any(|t| t.name == name && t.full_access)
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
            if input["on"].as_bool() == Some(true) { "on" } else { "off" },
            iliada_system::software::find(s(input, "name")).map(|i| i.name).unwrap_or(s(input, "name"))
        ),
        "update_iliada" => "Update Iliada".into(),
        "set_appearance" => {
            format!("Switch to {} appearance", if input["dark"].as_bool() == Some(true) { "dark" } else { "light" })
        }
        "set_volume" => match (input["percent"].as_u64(), input["muted"].as_bool()) {
            (Some(p), Some(true)) => format!("Set the volume to {p}% and mute it"),
            (Some(p), _) => format!("Set the volume to {p}%"),
            (None, Some(true)) => "Mute the sound".into(),
            (None, _) => "Unmute the sound".into(),
        },
        "open_link" => format!("Open {}", s(input, "url")),
        "run_command" => format!("Run the command: {}", s(input, "command")),
        "send_email" => {
            let cc = list(input, "cc");
            format!(
                "Send this email to {}{}\nSubject: {}\n\n{}",
                list(input, "to").join(", "),
                if cc.is_empty() { String::new() } else { format!(" (copy to {})", cc.join(", ")) },
                s(input, "subject"),
                s(input, "body")
            )
        }
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
        "add_tools" => {
            // Everyday names ("python") become the package's ("python3").
            let packages = list(input, "packages")
                .into_iter()
                .map(|p| match iliada_system::software::lookup(&p) {
                    Some(i) if i.kind == iliada_system::software::Kind::Software => i.id.to_owned(),
                    _ => p,
                })
                .collect();
            system(Request::AddTools { packages }).await?
        }
        "remove_tools" => system(Request::RemoveTools { packages: list(input, "packages") }).await?,
        "set_service" => {
            let on = input["on"].as_bool().context("\"on\" must be true or false")?;
            system(Request::SetService { name: s(input, "name").to_owned(), on }).await?
        }
        "list_software" => {
            use iliada_system::software::{CATALOG, Kind};
            let st = client::status().await?;
            Value::Array(
                CATALOG
                    .iter()
                    .map(|i| {
                        let on = match i.kind {
                            Kind::Software => st.tools.iter().any(|t| t == i.id),
                            Kind::Service => st.services.iter().any(|x| x.name == i.id && x.on),
                        };
                        json!({ "id": i.id, "name": i.name, "kind": i.kind, "summary": i.summary, "installed_or_on": on })
                    })
                    .collect(),
            )
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
        "set_appearance" => {
            let dark = input["dark"].as_bool().context("\"dark\" must be true or false")?;
            let dir = config_home()?.join("cosmic/com.system76.CosmicTheme.Mode/v1");
            std::fs::create_dir_all(&dir)?;
            // COSMIC watches its settings and switches at once.
            std::fs::write(dir.join("is_dark"), if dark { "true" } else { "false" })?;
            json!({ "appearance": if dark { "dark" } else { "light" } })
        }
        "set_volume" => {
            let (percent, muted) = (input["percent"].as_u64(), input["muted"].as_bool());
            anyhow::ensure!(percent.is_some() || muted.is_some(), "give a percent, or muted true/false");
            if let Some(p) = percent {
                wpctl(&["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{}%", p.min(100))]).await?;
            }
            if let Some(m) = muted {
                wpctl(&["set-mute", "@DEFAULT_AUDIO_SINK@", if m { "1" } else { "0" }]).await?;
            }
            let now =
                tokio::process::Command::new("wpctl").args(["get-volume", "@DEFAULT_AUDIO_SINK@"]).output().await?;
            json!({ "volume": String::from_utf8_lossy(&now.stdout).trim() })
        }
        "open_link" => {
            let url = s(input, "url").trim();
            anyhow::ensure!(
                ["https://", "http://", "mailto:"].iter().any(|p| url.starts_with(p))
                    && !url.chars().any(char::is_whitespace),
                "only http, https and mailto links can be opened"
            );
            tokio::process::Command::new("xdg-open")
                .arg(url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .context("opening the link")?;
            json!({ "opened": url })
        }
        "send_email" => {
            let email = crate::email::Email {
                to: list(input, "to"),
                cc: list(input, "cc"),
                subject: s(input, "subject").to_owned(),
                body: s(input, "body").to_owned(),
            };
            crate::email::send(&email).await?;
            json!({ "sent_to": email.to, "cc": email.cc, "subject": email.subject })
        }
        "run_command" => {
            let command = s(input, "command");
            anyhow::ensure!(!command.trim().is_empty(), "no command given");
            let secs = input["timeout_seconds"].as_u64().unwrap_or(120).clamp(1, 600);
            return run_command(command, std::time::Duration::from_secs(secs)).await;
        }
        other => bail!("there is no tool called {other}"),
    };
    Ok(out.to_string())
}

fn config_home() -> Result<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(x));
    }
    Ok(PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config"))
}

async fn wpctl(args: &[&str]) -> Result<()> {
    let st = tokio::process::Command::new("wpctl").args(args).status().await.context("changing the volume")?;
    anyhow::ensure!(st.success(), "the sound system refused the change");
    Ok(())
}

/// Most of a command's output that is passed back.
const MAX_OUTPUT: usize = 10_000;

/// A command run with bash as the user, in their home folder; its exit
/// status and (cut) output, stdout and stderr together.
pub async fn run_command(command: &str, timeout: std::time::Duration) -> Result<String> {
    use tokio::io::AsyncReadExt;
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_dir()).unwrap_or_else(|| "/".into());
    let mut child = tokio::process::Command::new("bash")
        .args(["-c", &format!("exec 2>&1\n{command}")])
        .current_dir(home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .process_group(0)
        .spawn()
        .context("starting bash")?;
    let mut out = child.stdout.take().expect("piped");
    let mut buf = Vec::new();
    let read = async {
        let mut chunk = [0u8; 8192];
        loop {
            match out.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) if buf.len() < MAX_OUTPUT * 4 => buf.extend_from_slice(&chunk[..n]),
                Ok(_) => {}
            }
        }
        child.wait().await
    };
    let status = match tokio::time::timeout(timeout, read).await {
        Ok(st) => st.ok().and_then(|s| s.code()).map(|c| c.to_string()).unwrap_or_else(|| "killed".into()),
        Err(_) => {
            if let Some(pid) = child.id() {
                // The whole group: bash and whatever it started.
                let _ = std::process::Command::new("kill").args(["-KILL", &format!("-{pid}")]).status();
            }
            format!("stopped after {} seconds", timeout.as_secs())
        }
    };
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if let Some((i, _)) = text.char_indices().nth(MAX_OUTPUT) {
        text.truncate(i);
        text.push_str("\n… (output cut)");
    }
    Ok(json!({ "exit": status, "output": text }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn commands_report_exit_status_and_output_and_stop_on_time() {
        let r: Value = serde_json::from_str(
            &run_command("echo out; echo err >&2; exit 3", std::time::Duration::from_secs(10)).await.unwrap(),
        )
        .unwrap();
        assert_eq!(r["exit"], "3");
        assert_eq!(r["output"], "out\nerr\n");
        let t = std::time::Instant::now();
        let r: Value =
            serde_json::from_str(&run_command("sleep 30", std::time::Duration::from_secs(1)).await.unwrap()).unwrap();
        assert!(r["exit"].as_str().unwrap().starts_with("stopped"), "{r}");
        assert!(t.elapsed() < std::time::Duration::from_secs(10));
        let r: Value = serde_json::from_str(
            &run_command("yes | head -c 50000", std::time::Duration::from_secs(10)).await.unwrap(),
        )
        .unwrap();
        assert!(r["output"].as_str().unwrap().ends_with("(output cut)"));
    }

    #[test]
    fn full_access_tools_only_with_full_access() {
        let names = |f| -> Vec<String> {
            definitions(f).as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_owned()).collect()
        };
        assert!(!names(false).contains(&"run_command".to_owned()));
        assert!(names(true).contains(&"run_command".to_owned()));
        assert!(needs_full_access("run_command") && !needs_full_access("set_appearance"));
        assert_eq!(describe("run_command", &json!({ "command": "ls ~" })), "Run the command: ls ~");
    }

    #[test]
    fn tools_are_well_formed_and_actions_are_marked() {
        let defs = definitions(true);
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
        assert_eq!(describe("set_service", &json!({ "name": "ssh", "on": true })), "Turn on SSH server");
    }
}

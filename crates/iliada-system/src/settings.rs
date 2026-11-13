//! `/etc/iliada/settings.toml`: system settings in plain TOML (no Nix).
//!
//! ```toml
//! [tools]
//! packages = ["vim", "git"]   # command-line tools (attribute names in nixpkgs)
//!
//! [services]
//! ssh = false                  # SSH server
//! printing = false
//! ```
//!
//! The Iliada NixOS modules read this file (`iliada.settingsFile`); the
//! system service edits it for the `iliada` command and Settings. Edits keep
//! comments.

use anyhow::{Result, bail};
use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

pub const SERVICES: &[(&str, &str)] = &[
    ("ssh", "SSH server (remote login)"),
    ("printing", "Printing (CUPS)"),
    ("nginx", "nginx web server (/srv/www)"),
    ("postgresql", "PostgreSQL database"),
    ("mariadb", "MariaDB database"),
    ("redis", "Redis"),
    ("docker", "Docker"),
];

/// The systemd unit behind each service (what `systemctl` calls it).
pub const UNITS: &[(&str, &str)] = &[
    ("ssh", "sshd"),
    ("printing", "cups"),
    ("nginx", "nginx"),
    ("postgresql", "postgresql"),
    ("mariadb", "mysql"),
    ("redis", "redis"),
    ("docker", "docker"),
];

/// The service a systemd unit name belongs to: "sshd", "sshd.service",
/// "mysql" or an everyday name ("ssh", "postgres").
pub fn service_of_unit(unit: &str) -> Option<&'static str> {
    let u = unit.strip_suffix(".service").or_else(|| unit.strip_suffix(".socket")).unwrap_or(unit);
    UNITS
        .iter()
        .find(|(_, x)| *x == u)
        .map(|(s, _)| *s)
        .or_else(|| crate::software::lookup(u).filter(|i| i.kind == crate::software::Kind::Service).map(|i| i.id))
}

pub fn unit_of_service(service: &str) -> Option<&'static str> {
    UNITS.iter().find(|(s, _)| *s == service).map(|(_, u)| *u)
}

fn tools_array(doc: &mut DocumentMut) -> &mut Array {
    if !doc.contains_key("tools") {
        doc["tools"] = Item::Table(Table::new());
    }
    let t = doc["tools"].as_table_mut().expect("[tools] is a table");
    if !t.contains_key("packages") {
        t["packages"] = value(Array::new());
    }
    t["packages"].as_array_mut().expect("tools.packages is an array")
}

pub fn tools(doc: &DocumentMut) -> Vec<String> {
    doc.get("tools")
        .and_then(|t| t.get("packages"))
        .and_then(Item::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Valid nixpkgs attribute paths only ("vim", "python3Packages.requests").
pub fn valid_tool_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_+".contains(c)))
}

pub fn add_tools(doc: &mut DocumentMut, names: &[String]) -> Result<Vec<String>> {
    for n in names {
        if !valid_tool_name(n) {
            bail!("{n:?} is not a valid package name");
        }
    }
    let have = tools(doc);
    let arr = tools_array(doc);
    let mut added = Vec::new();
    for n in names {
        if !have.contains(n) && !added.contains(n) {
            arr.push(n.as_str());
            added.push(n.clone());
        }
    }
    Ok(added)
}

pub fn remove_tools(doc: &mut DocumentMut, names: &[String]) -> Vec<String> {
    let arr = tools_array(doc);
    let mut removed = Vec::new();
    arr.retain(|v| match v.as_str() {
        Some(s) if names.iter().any(|n| n == s) => {
            removed.push(s.to_owned());
            false
        }
        _ => true,
    });
    removed
}

/// `[assistant]`: enabled, mode ("ask" / "auto"), model, full_access.
pub fn assistant(doc: &DocumentMut) -> crate::protocol::AssistantSettings {
    use crate::protocol::{AssistantMode, AssistantSettings};
    let d = AssistantSettings::default();
    let Some(t) = doc.get("assistant") else { return d };
    AssistantSettings {
        enabled: t.get("enabled").and_then(Item::as_bool).unwrap_or(d.enabled),
        mode: match t.get("mode").and_then(Item::as_str) {
            Some("auto") => AssistantMode::Auto,
            _ => AssistantMode::Ask,
        },
        model: t.get("model").and_then(Item::as_str).map(str::to_owned).unwrap_or(d.model),
        full_access: t.get("full_access").and_then(Item::as_bool).unwrap_or(false),
    }
}

/// A model id: letters, digits and `-._:/` ("claude-opus-5-5",
/// "deepseek/deepseek-v4-flash").
pub fn valid_model(s: &str) -> bool {
    (1..=96).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "-._:/".contains(c))
        && !s.starts_with(['/', '.'])
}

pub fn set_assistant(doc: &mut DocumentMut, a: &crate::protocol::AssistantSettings) -> Result<bool> {
    if !valid_model(&a.model) {
        bail!("{:?} is not a model name", a.model);
    }
    if assistant(doc) == *a {
        return Ok(false);
    }
    if !doc.contains_key("assistant") {
        doc["assistant"] = Item::Table(Table::new());
    }
    doc["assistant"]["enabled"] = value(a.enabled);
    doc["assistant"]["mode"] = value(match a.mode {
        crate::protocol::AssistantMode::Ask => "ask",
        crate::protocol::AssistantMode::Auto => "auto",
    });
    doc["assistant"]["model"] = value(a.model.as_str());
    doc["assistant"]["full_access"] = value(a.full_access);
    Ok(true)
}

/// How a service is set up: three levels, as on Debian or Fedora.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// Not on this computer.
    Off,
    /// Installed; starts only when asked (`systemctl start`).
    Manual,
    /// Installed and started at every boot.
    On,
}

/// `ssh = true` (or `"on"`), `ssh = "manual"`, or `false` / absent.
pub fn service_state(doc: &DocumentMut, name: &str) -> ServiceState {
    match doc.get("services").and_then(|s| s.get(name)) {
        Some(i) if i.as_bool() == Some(true) || i.as_str() == Some("on") => ServiceState::On,
        Some(i) if i.as_str() == Some("manual") => ServiceState::Manual,
        _ => ServiceState::Off,
    }
}

/// The service is installed (on, or manual).
pub fn service(doc: &DocumentMut, name: &str) -> bool {
    service_state(doc, name) != ServiceState::Off
}

pub fn set_service_state(doc: &mut DocumentMut, name: &str, state: ServiceState) -> Result<bool> {
    if !SERVICES.iter().any(|(n, _)| *n == name) {
        bail!("unknown service {name:?} (known: {})", SERVICES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "));
    }
    if service_state(doc, name) == state {
        return Ok(false);
    }
    if !doc.contains_key("services") {
        doc["services"] = Item::Table(Table::new());
    }
    doc["services"][name] = match state {
        ServiceState::On => value(true),
        ServiceState::Manual => value("manual"),
        ServiceState::Off => value(false),
    };
    Ok(true)
}

/// Install (`on`: installed and started at boot, unless it is installed
/// already) or remove a service.
pub fn set_service(doc: &mut DocumentMut, name: &str, on: bool) -> Result<bool> {
    match (on, service_state(doc, name)) {
        (true, ServiceState::Off) => set_service_state(doc, name, ServiceState::On),
        (true, _) => set_service_state(doc, name, service_state(doc, name)),
        (false, _) => set_service_state(doc, name, ServiceState::Off),
    }
}

/// Start at boot or not (`systemctl enable` / `disable`). Enabling a
/// service that isn't installed installs it.
pub fn set_service_boot(doc: &mut DocumentMut, name: &str, boot: bool) -> Result<bool> {
    let next = match (boot, service_state(doc, name)) {
        (true, _) => ServiceState::On,
        (false, ServiceState::On) => ServiceState::Manual,
        (false, s) => s,
    };
    set_service_state(doc, name, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_map_to_services() {
        for (s, _) in SERVICES {
            assert!(unit_of_service(s).is_some(), "{s} has no unit");
        }
        assert_eq!(service_of_unit("sshd"), Some("ssh"));
        assert_eq!(service_of_unit("sshd.service"), Some("ssh"));
        assert_eq!(service_of_unit("ssh"), Some("ssh"));
        assert_eq!(service_of_unit("mysql"), Some("mariadb"));
        assert_eq!(service_of_unit("docker.socket"), Some("docker"));
        assert_eq!(service_of_unit("postgres"), Some("postgresql"));
        assert_eq!(service_of_unit("NetworkManager"), None);
        assert_eq!(service_of_unit("python"), None);
    }

    #[test]
    fn edits_keep_comments_and_are_idempotent() {
        let mut doc: DocumentMut = "# my settings\n[tools]\npackages = [\"vim\"] # editors\n".parse().unwrap();
        assert_eq!(add_tools(&mut doc, &["git".into(), "vim".into()]).unwrap(), vec!["git".to_string()]);
        assert_eq!(tools(&doc), vec!["vim", "git"]);
        assert_eq!(remove_tools(&mut doc, &["vim".into()]), vec!["vim".to_string()]);
        assert!(set_service(&mut doc, "ssh", true).unwrap());
        assert!(!set_service(&mut doc, "ssh", true).unwrap());
        assert!(set_service(&mut doc, "telnet", true).is_err());
        // The three levels: on, manual (installed, not at boot), off.
        assert_eq!(service_state(&doc, "ssh"), ServiceState::On);
        assert!(set_service_boot(&mut doc, "ssh", false).unwrap());
        assert_eq!(service_state(&doc, "ssh"), ServiceState::Manual);
        assert!(doc.to_string().contains("ssh = \"manual\""));
        assert!(!set_service(&mut doc, "ssh", true).unwrap(), "installing an installed service changes nothing");
        assert!(!set_service_boot(&mut doc, "nginx", false).unwrap(), "not installed: nothing to disable");
        assert!(set_service_boot(&mut doc, "nginx", true).unwrap());
        assert_eq!(service_state(&doc, "nginx"), ServiceState::On);
        assert!(set_service(&mut doc, "ssh", false).unwrap());
        assert_eq!(service_state(&doc, "ssh"), ServiceState::Off);
        let s = doc.to_string();
        assert!(s.starts_with("# my settings"));
        assert!(s.contains("nginx = true") && s.contains("ssh = false"));
        assert!(add_tools(&mut doc, &["rm -rf /".into()]).is_err());

        use crate::protocol::{AssistantMode, AssistantSettings};
        assert!(!assistant(&doc).enabled);
        let a = AssistantSettings {
            enabled: true,
            mode: AssistantMode::Auto,
            model: "claude-opus-5-5".into(),
            full_access: false,
        };
        assert!(set_assistant(&mut doc, &a).unwrap());
        assert_eq!(assistant(&doc), a);
        assert!(!set_assistant(&mut doc, &a).unwrap());
        let other = AssistantSettings { model: "deepseek/deepseek-v4-flash".into(), full_access: true, ..a };
        assert!(set_assistant(&mut doc, &other).unwrap());
        assert_eq!(assistant(&doc), other);
        assert_eq!(other.effective_mode(), AssistantMode::Auto, "the user's choice");
        assert!(!other.recommended_for_auto());
        assert!(set_assistant(&mut doc, &AssistantSettings { model: "a b".into(), ..other.clone() }).is_err());
        assert!(set_assistant(&mut doc, &AssistantSettings { model: "/etc".into(), ..other }).is_err());
    }
}

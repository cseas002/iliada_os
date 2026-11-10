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

pub const SERVICES: &[(&str, &str)] = &[("ssh", "SSH server (remote login)"), ("printing", "Printing (CUPS)")];

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

/// `[assistant]`: enabled, mode ("ask" / "auto"), model.
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
    }
}

/// A Claude model id: letters, digits, '-' and '.'.
pub fn valid_model(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
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
    Ok(true)
}

pub fn service(doc: &DocumentMut, name: &str) -> bool {
    doc.get("services").and_then(|s| s.get(name)).and_then(Item::as_bool).unwrap_or(false)
}

pub fn set_service(doc: &mut DocumentMut, name: &str, on: bool) -> Result<bool> {
    if !SERVICES.iter().any(|(n, _)| *n == name) {
        bail!("unknown service {name:?} (known: {})", SERVICES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "));
    }
    if service(doc, name) == on {
        return Ok(false);
    }
    if !doc.contains_key("services") {
        doc["services"] = Item::Table(Table::new());
    }
    doc["services"][name] = value(on);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_comments_and_are_idempotent() {
        let mut doc: DocumentMut = "# my settings\n[tools]\npackages = [\"vim\"] # editors\n".parse().unwrap();
        assert_eq!(add_tools(&mut doc, &["git".into(), "vim".into()]).unwrap(), vec!["git".to_string()]);
        assert_eq!(tools(&doc), vec!["vim", "git"]);
        assert_eq!(remove_tools(&mut doc, &["vim".into()]), vec!["vim".to_string()]);
        assert!(set_service(&mut doc, "ssh", true).unwrap());
        assert!(!set_service(&mut doc, "ssh", true).unwrap());
        assert!(set_service(&mut doc, "telnet", true).is_err());
        let s = doc.to_string();
        assert!(s.starts_with("# my settings"));
        assert!(s.contains("ssh = true"));
        assert!(add_tools(&mut doc, &["rm -rf /".into()]).is_err());

        use crate::protocol::{AssistantMode, AssistantSettings};
        assert!(!assistant(&doc).enabled);
        let a = AssistantSettings { enabled: true, mode: AssistantMode::Auto, model: "claude-opus-5-5".into() };
        assert!(set_assistant(&mut doc, &a).unwrap());
        assert_eq!(assistant(&doc), a);
        assert!(!set_assistant(&mut doc, &a).unwrap());
        let weak = AssistantSettings { model: "claude-haiku-5-5".into(), ..a };
        assert_eq!(weak.effective_mode(), AssistantMode::Ask, "Auto needs a trusted model");
    }
}

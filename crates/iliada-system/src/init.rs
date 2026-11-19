//! Creating `/etc/iliada`: the system's configuration, a git repository.
//!
//! Used by the installer (onto `/mnt`) and on the first boot of the
//! ready-to-run VM image (onto `/`). It copies the Iliada source the system
//! is built from into `/etc/iliada/source`, writes the configuration from the
//! template, and commits it.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// What kind of computer this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Real hardware: `nixos-generate-config` has written
    /// `<root>/etc/nixos/hardware-configuration.nix`.
    Hardware,
    /// A virtual machine started from the ready-to-run image.
    Vm,
}

#[derive(Debug, Clone)]
pub struct InitOptions {
    /// `/` on a running system, `/mnt` in the installer.
    pub root: PathBuf,
    /// The Iliada source to build from (a store path or a checkout).
    pub source: PathBuf,
    pub profile: Profile,
    pub user: String,
    pub full_name: String,
    pub hostname: String,
    pub timezone: String,
    pub auto_login: bool,
    /// Keep developer SSH access (the utm-dev image).
    pub dev_access: bool,
    /// `x86_64-linux` or `aarch64-linux`.
    pub system: String,
}

pub fn valid_user(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some('a'..='z' | '_'))
        && s.len() <= 31
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

pub fn valid_hostname(s: &str) -> bool {
    (1..=63).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

pub fn valid_timezone(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "/_+-".contains(c))
        && !s.contains("..")
}

/// A full name safe inside a Nix string (no quotes, escapes or `${`).
fn nix_text(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '"' | '\\' | '$') && !c.is_control()).collect()
}

/// The machine's Nix system name.
pub fn host_system() -> String {
    format!("{}-linux", std::env::consts::ARCH)
}

const VM_HARDWARE: &str = r#"# This computer is a virtual machine (UTM, QEMU, Proxmox…) started from the
# ready-to-run Iliada image: its disks, boot loader and guest tools.
{ iliada, ... }:
{
  imports = [ iliada.nixosModules.vm-guest ];
}
"#;

const GITIGNORE: &str = "# Not configuration: the bundled Iliada source, and files the system manages.\nsource/\nisolation.json\nstore.json\n";

fn etc(root: &Path) -> PathBuf {
    root.join("etc/iliada")
}

/// Run git in `dir` as Iliada, logging its output.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Iliada", "-c", "user.email=iliada@localhost", "-c", "commit.gpgsign=false"])
        // Every object and ref on the disk before git says it's done: by
        // default git leaves new objects in the cache, and a power cut
        // after a commit left an empty object that stopped all updates.
        .args(["-c", "core.fsync=all", "-c", "core.fsyncMethod=fsync"])
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Mend the history of `dir` if a crash or power cut damaged it (empty or
/// missing objects: then Nix can't read the configuration and nothing can
/// be applied or updated). Goes back to the newest commit that is whole,
/// keeping the files as they are; the next commit records them again.
/// `Ok(true)`: something was mended.
pub fn repair(dir: &Path) -> Result<bool> {
    let git_dir = dir.join(".git");
    if !git_dir.exists() || git(dir, &["fsck", "--no-progress", "--no-dangling"]).is_ok() {
        return Ok(false);
    }
    // Empty object files are what an interrupted write leaves: never valid.
    remove_empty_files(&git_dir.join("objects"));
    let whole = |c: &str| git(dir, &["rev-list", "--objects", c]).is_ok();
    // The branch's own log, newest last: every commit it has pointed at.
    let log = std::fs::read_to_string(git_dir.join("logs/refs/heads/main")).unwrap_or_default();
    let good = log.lines().rev().filter_map(|l| l.split(' ').nth(1)).find(|c| whole(c)).map(str::to_owned);
    let _ = std::fs::remove_file(git_dir.join("index"));
    match good {
        Some(commit) => {
            git(dir, &["update-ref", "refs/heads/main", &commit])?;
            git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
            // The logs still name the lost commits, which fsck counts as damage.
            git(dir, &["reflog", "expire", "--expire=now", "--all"])?;
            // The index again from that commit; the files stay as they are.
            git(dir, &["reset", "-q"])?;
        }
        None => {
            // Nothing whole is left: keep the broken history aside, start anew.
            let aside = dir.join(format!(".git-damaged-{}", now_secs()));
            std::fs::rename(&git_dir, &aside)?;
            git(dir, &["init", "-q", "-b", "main"])?;
            // Kept for a person to look at, never part of the configuration.
            std::fs::write(git_dir.join("info/exclude"), "/.git-damaged-*\n")?;
        }
    }
    commit_all(dir, "Repaired after an unexpected shutdown")?;
    Ok(true)
}

fn remove_empty_files(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => remove_empty_files(&p),
            Ok(t) if t.is_file() && e.metadata().is_ok_and(|m| m.len() == 0) => {
                let _ = std::fs::remove_file(&p);
            }
            _ => {}
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Commit everything that changed (nothing if nothing did). A system set
/// up without the installer (a development VM) gets its history started.
pub fn commit_all(dir: &Path, message: &str) -> Result<bool> {
    if !dir.join(".git").exists() {
        git(dir, &["init", "-q", "-b", "main"])?;
    }
    git(dir, &["add", "-A"])?;
    if git(dir, &["status", "--porcelain"])?.trim().is_empty() {
        return Ok(false);
    }
    git(dir, &["commit", "-q", "-m", message])?;
    Ok(true)
}

/// Copy a directory tree, following symlinks, leaving it writable by root.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let meta = std::fs::metadata(&src)?;
        if meta.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).with_context(|| format!("copying {}", src.display()))?;
            let mode = meta.permissions().mode() | 0o200;
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode))?;
        }
    }
    let mode = std::fs::metadata(from)?.permissions().mode() | 0o700;
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// The source path as written into the flake: where it is *during* the
/// build (under the root), rewritten by [`finish`] when the root isn't `/`.
fn source_url(root: &Path) -> String {
    format!("path:{}", etc(root).join("source").display())
}

pub fn configuration(template: &str, o: &InitOptions) -> String {
    let mut out = String::with_capacity(template.len());
    for line in template.lines() {
        let t = line.trim_start();
        let indent = &line[..line.len() - t.len()];
        let line = if t.starts_with("networking.hostName = ") {
            format!("{indent}networking.hostName = \"{}\";", o.hostname)
        } else if t.starts_with("time.timeZone = ") {
            format!("{indent}time.timeZone = \"{}\"; # e.g. \"Europe/Athens\"", o.timezone)
        } else if t == "users.users.iliada = {" {
            format!("{indent}users.users.{} = {{", o.user)
        } else if t.starts_with("description = \"Iliada User\";") {
            format!("{indent}description = \"{}\";", nix_text(&o.full_name))
        } else if t.starts_with("initialPassword = ") {
            continue;
        } else if t.starts_with("# desktop.autoLoginUser = ") && o.auto_login {
            format!("{indent}desktop.autoLoginUser = \"{}\";", o.user)
        } else {
            line.to_owned()
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub fn flake(template: &str, o: &InitOptions) -> String {
    let mut out = String::with_capacity(template.len());
    for line in template.lines() {
        let t = line.trim_start();
        let indent = &line[..line.len() - t.len()];
        let line = if t.starts_with("inputs.iliada.url = ") {
            format!("{indent}inputs.iliada.url = \"{}\";", source_url(&o.root))
        } else if t.starts_with("system = ") {
            format!("{indent}system = \"{}\";", o.system)
        } else {
            line.to_owned()
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Write and commit `<root>/etc/iliada`.
pub fn init(o: &InitOptions, log: &mut dyn FnMut(&str)) -> Result<()> {
    if !valid_user(&o.user) {
        bail!("{:?} is not a valid user name (lowercase letters, digits, - and _)", o.user);
    }
    if !valid_hostname(&o.hostname) {
        bail!("{:?} is not a valid computer name (letters, digits and -)", o.hostname);
    }
    if !valid_timezone(&o.timezone) {
        bail!("{:?} is not a valid time zone", o.timezone);
    }
    let dir = etc(&o.root);
    if dir.join("flake.nix").exists() {
        bail!("{} is already set up", dir.display());
    }
    let template = o.source.join("templates/default");
    if !template.join("configuration.nix").exists() {
        bail!("{} is not an Iliada source (no templates/default)", o.source.display());
    }
    std::fs::create_dir_all(&dir)?;

    log("Copying the Iliada source");
    let source = dir.join("source");
    if source.exists() || source.is_symlink() {
        if source.is_symlink() || source.is_file() {
            std::fs::remove_file(&source)?;
        } else {
            std::fs::remove_dir_all(&source)?;
        }
    }
    copy_tree(&o.source, &source)?;

    log("Writing the configuration");
    let read = |name: &str| std::fs::read_to_string(template.join(name)).with_context(|| format!("template {name}"));
    std::fs::write(dir.join("flake.nix"), flake(&read("flake.nix")?, o))?;
    std::fs::write(dir.join("configuration.nix"), configuration(&read("configuration.nix")?, o))?;
    for name in ["settings.toml", "apps.toml"] {
        if !dir.join(name).exists() {
            std::fs::write(dir.join(name), read(name)?)?;
        }
    }
    let hardware = dir.join("hardware-configuration.nix");
    match o.profile {
        Profile::Hardware => {
            let generated = o.root.join("etc/nixos/hardware-configuration.nix");
            std::fs::rename(&generated, &hardware)
                .with_context(|| format!("moving {} (run nixos-generate-config first)", generated.display()))?;
            let _ = std::fs::remove_dir_all(o.root.join("etc/nixos"));
        }
        Profile::Vm => {
            let text = if o.dev_access {
                VM_HARDWARE.replace(
                    "imports = [ iliada.nixosModules.vm-guest ];",
                    "imports = [ iliada.nixosModules.vm-guest iliada.nixosModules.dev-access ];",
                )
            } else {
                VM_HARDWARE.to_owned()
            };
            std::fs::write(&hardware, text)?
        }
    }
    std::fs::write(dir.join(".gitignore"), GITIGNORE)?;

    log("Recording the configuration");
    if !dir.join(".git").exists() {
        git(&dir, &["init", "-q", "-b", "main"])?;
    }
    commit_all(&dir, &format!("Initial configuration for {}", o.hostname))?;
    // Git flakes hash only committed files, so the lock file doesn't
    // disturb the flake it locks.
    let url = format!("git+file://{}", dir.display());
    let out = Command::new("nix")
        .args(["--extra-experimental-features", "nix-command flakes", "flake", "lock", &url])
        .output()
        .context("running nix flake lock")?;
    if !out.status.success() {
        bail!("locking the configuration: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    commit_all(&dir, "Lock Iliada")?;
    Ok(())
}

/// After installing onto a root other than `/`: point the flake at the
/// source where the installed system will see it.
pub fn finish(root: &Path) -> Result<()> {
    let dir = etc(root);
    let (during, after) = (source_url(root), source_url(Path::new("/")));
    let during_path = during.trim_start_matches("path:");
    let after_path = after.trim_start_matches("path:");
    for name in ["flake.nix", "flake.lock"] {
        let p = dir.join(name);
        let text = std::fs::read_to_string(&p)?;
        std::fs::write(
            &p,
            text.replace(&during, &after).replace(&format!("\"{during_path}\""), &format!("\"{after_path}\"")),
        )?;
    }
    commit_all(&dir, "Installed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> InitOptions {
        InitOptions {
            root: "/mnt".into(),
            source: "/src".into(),
            profile: Profile::Vm,
            user: "alex".into(),
            full_name: "Alex \"${evil}\" P".into(),
            hostname: "alex-laptop".into(),
            timezone: "Europe/Athens".into(),
            auto_login: true,
            dev_access: false,
            system: "aarch64-linux".into(),
        }
    }

    #[test]
    fn writes_the_template_for_this_computer() {
        let tpl =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../templates/default/configuration.nix"))
                .unwrap();
        let c = configuration(&tpl, &opts());
        assert!(c.contains("networking.hostName = \"alex-laptop\";"));
        assert!(c.contains("time.timeZone = \"Europe/Athens\";"));
        assert!(c.contains("users.users.alex = {"));
        assert!(c.contains("description = \"Alex {evil} P\";"));
        assert!(c.contains("desktop.autoLoginUser = \"alex\";"));
        assert!(!c.contains("initialPassword"));

        let tpl =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../templates/default/flake.nix")).unwrap();
        let f = flake(&tpl, &opts());
        assert!(f.contains("inputs.iliada.url = \"path:/mnt/etc/iliada/source\";"));
        assert!(f.contains("system = \"aarch64-linux\";"));
    }

    #[test]
    fn validates_names() {
        assert!(
            valid_user("alex") && valid_user("_x-1") && !valid_user("Alex") && !valid_user("1a") && !valid_user("")
        );
        assert!(valid_hostname("my-pc") && !valid_hostname("-x") && !valid_hostname("a b"));
        assert!(valid_timezone("America/New_York") && !valid_timezone("../etc") && !valid_timezone("a b"));
    }
}

#[cfg(test)]
mod repair_tests {
    use super::*;

    /// A power cut right after a commit: the commit's object file is empty.
    #[test]
    fn mends_a_history_a_power_cut_broke() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("settings.toml"), "a = 1\n").unwrap();
        commit_all(d, "first").unwrap();
        std::fs::write(d.join("settings.toml"), "a = 2\n").unwrap();
        commit_all(d, "second").unwrap();
        assert!(!repair(d).unwrap(), "a whole history needs nothing");

        let head = git(d, &["rev-parse", "HEAD"]).unwrap();
        let obj = d.join(".git/objects").join(&head[..2]).join(head[2..].trim());
        let mut perms = std::fs::metadata(&obj).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&obj, perms).unwrap();
        std::fs::write(&obj, "").unwrap();
        assert!(git(d, &["fsck", "--no-progress"]).is_err(), "the history is broken now");

        assert!(repair(d).unwrap());
        git(d, &["fsck", "--no-progress", "--no-dangling"]).unwrap();
        // The files are as they were, and recorded again.
        assert_eq!(std::fs::read_to_string(d.join("settings.toml")).unwrap(), "a = 2\n");
        assert_eq!(git(d, &["show", "HEAD:settings.toml"]).unwrap(), "a = 2\n");
        let log = git(d, &["log", "--format=%s"]).unwrap();
        assert_eq!(log.lines().collect::<Vec<_>>(), ["Repaired after an unexpected shutdown", "first"]);
    }

    #[test]
    fn starts_anew_when_nothing_is_whole() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("apps.toml"), "x\n").unwrap();
        commit_all(d, "only").unwrap();
        // Every object emptied.
        for e in walk(&d.join(".git/objects")) {
            let mut p = std::fs::metadata(&e).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            std::fs::set_permissions(&e, p).unwrap();
            std::fs::write(&e, "").unwrap();
        }
        assert!(repair(d).unwrap());
        git(d, &["fsck", "--no-progress", "--no-dangling"]).unwrap();
        assert_eq!(git(d, &["show", "HEAD:apps.toml"]).unwrap(), "x\n");
        assert!(
            std::fs::read_dir(d)
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(".git-damaged-"))
        );
        // Set aside, and never committed (backups would carry it).
        assert!(!git(d, &["ls-files"]).unwrap().contains(".git-damaged"));
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
        }
        out
    }
}

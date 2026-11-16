//! `iliada report`: one plain text file describing this computer's hardware
//! and how Iliada gets on with it (graphics driver, displays and scale,
//! Wi-Fi, sound, suspend, battery, missing firmware, errors since start-up).
//!
//! Made for a first boot on a new PC: save it, read it, and pass it to
//! whoever helps. It runs as the person (no administrator password) and
//! leaves out serial numbers and network addresses; the journal lines it
//! includes can still name this computer and its Wi-Fi network.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

/// How long any one command may take before it is skipped.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Longest output kept from one command.
const MAX_LINES: usize = 150;

/// Writes the report and returns where it went.
pub async fn write(out: Option<PathBuf>) -> Result<PathBuf> {
    let text = collect().await;
    let path = match out {
        Some(p) => p,
        None => default_path().await,
    };
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

async fn default_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let desktop = home.join("Desktop");
    let dir = if desktop.is_dir() { desktop } else { home };
    let stamp = run("date", &["+%Y-%m-%d %H.%M"]).await.unwrap_or_default();
    let stamp = stamp.trim();
    dir.join(if stamp.is_empty() { "Iliada report.txt".to_string() } else { format!("Iliada report {stamp}.txt") })
}

async fn collect() -> String {
    let mut r = String::new();
    let _ = writeln!(r, "Iliada report");
    let _ = writeln!(r, "=============");
    let _ = writeln!(r, "Made by `iliada report`. Plain text: read it before you share it.\n");

    section(&mut r, "System");
    let os = read("/etc/os-release");
    // VERSION_ID is the NixOS release Iliada is built on.
    let base = os
        .lines()
        .find_map(|l| l.strip_prefix("VERSION_ID="))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap_or_default();
    line(&mut r, "Iliada", &format!("{} (base: NixOS {base})", env!("CARGO_PKG_VERSION")));
    line(&mut r, "Kernel", run("uname", &["-r"]).await.unwrap_or_default().trim());
    let dmi = |f: &str| read(&format!("/sys/class/dmi/id/{f}")).trim().to_string();
    line(&mut r, "Maker", &dmi("sys_vendor"));
    line(&mut r, "Model", format!("{} {}", dmi("product_name"), dmi("product_version")).trim());
    line(&mut r, "Board", &dmi("board_name"));
    line(&mut r, "Firmware", &format!("{} ({})", dmi("bios_version"), dmi("bios_date")));
    line(&mut r, "Started with", &boot_mode());
    line(&mut r, "Up", read("/proc/uptime").split_whitespace().next().map(uptime).as_deref().unwrap_or(""));

    section(&mut r, "Processor and memory");
    let cpu = read("/proc/cpuinfo");
    let field = |k: &str| {
        cpu.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_string())
            .unwrap_or_default()
    };
    line(&mut r, "Processor", &field("model name"));
    line(&mut r, "Threads", &cpu.lines().filter(|l| l.starts_with("processor")).count().to_string());
    line(&mut r, "Microcode", &field("microcode"));
    let mem = read("/proc/meminfo");
    let kb = |k: &str| {
        mem.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    if let Some(t) = kb("MemTotal:") {
        line(&mut r, "Memory", &format!("{:.1} GB", t as f64 / 1_048_576.0));
    }

    section(&mut r, "Graphics");
    graphics(&mut r);
    cmd(&mut r, "Displays as COSMIC sees them (scale)", "cosmic-randr", &["list"]).await;
    journal(
        &mut r,
        "Compositor messages",
        &["--user", "-b", "-o", "cat", "-g", "(?i)renderer|egl|gles|vulkan|drm|gpu|scale"],
        40,
    )
    .await;

    section(&mut r, "Network");
    net(&mut r);
    cmd(&mut r, "Network devices", "nmcli", &["-f", "DEVICE,TYPE,STATE", "device"]).await;
    cmd(&mut r, "Radios", "rfkill", &["list"]).await;

    section(&mut r, "Sound");
    let cards = read("/proc/asound/cards");
    block(&mut r, "Sound cards", if cards.trim().is_empty() { "(none found)" } else { &cards });
    cmd(&mut r, "PipeWire", "wpctl", &["status"]).await;

    section(&mut r, "Power and sleep");
    power(&mut r);
    line(&mut r, "Sleep kinds", read("/sys/power/mem_sleep").trim());
    cmd(&mut r, "Power mode", "powerprofilesctl", &["get"]).await;
    journal(
        &mut r,
        "Sleep since start-up",
        &[
            "-b",
            "-o",
            "short-monotonic",
            "-g",
            "PM: suspend|PM: resume|suspend exit|Suspending system|System returned from sleep|Failed to suspend",
        ],
        40,
    )
    .await;

    section(&mut r, "Input");
    let inputs: Vec<String> = read("/proc/bus/input/devices")
        .lines()
        .filter_map(|l| l.strip_prefix("N: Name="))
        .map(|n| format!("  {}", n.trim_matches('"')))
        .collect();
    block(&mut r, "Input devices", &inputs.join("\n"));

    section(&mut r, "Devices");
    cmd(&mut r, "PCI (with the driver in use)", "lspci", &["-nnk"]).await;
    cmd(&mut r, "USB", "lsusb", &[]).await;

    section(&mut r, "Problems");
    journal(
        &mut r,
        "Missing firmware",
        &["-k", "-b", "-o", "cat", "-g", "(?i)firmware.*(fail|error|not found|missing)|direct firmware load"],
        40,
    )
    .await;
    cmd(&mut r, "Services that failed", "systemctl", &["--failed", "--no-legend", "--plain"]).await;
    cmd(&mut r, "Your services that failed", "systemctl", &["--user", "--failed", "--no-legend", "--plain"]).await;
    journal(&mut r, "Errors since start-up", &["-b", "-p", "3", "-o", "short-monotonic"], 80).await;
    journal(&mut r, "Errors in your session", &["--user", "-b", "-p", "3", "-o", "short-monotonic"], 40).await;
    journal(&mut r, "Kernel warnings", &["-k", "-b", "-p", "4", "-o", "short-monotonic"], 60).await;
    r
}

fn section(r: &mut String, title: &str) {
    let _ = writeln!(r, "\n## {title}");
}

fn line(r: &mut String, k: &str, v: &str) {
    let v = v.trim();
    let _ = writeln!(r, "{k:<14} {}", if v.is_empty() { "(unknown)" } else { v });
}

fn block(r: &mut String, title: &str, body: &str) {
    let _ = writeln!(r, "\n{title}:");
    let body = body.trim_end();
    if body.is_empty() {
        let _ = writeln!(r, "  (nothing)");
    } else {
        let _ = writeln!(r, "{}", clip(body));
    }
}

fn clip(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= MAX_LINES {
        return s.to_string();
    }
    let skip = lines.len() - MAX_LINES;
    format!("  ({skip} earlier lines left out)\n{}", lines[skip..].join("\n"))
}

fn read(p: &str) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// Runs a program; None if it is missing, failed to start or took too long.
async fn run(prog: &str, args: &[&str]) -> Option<String> {
    let mut c = tokio::process::Command::new(prog);
    // Run from a terminal outside the desktop (SSH, a text console), find
    // the desktop so COSMIC can still be asked about the screens.
    if std::env::var_os("WAYLAND_DISPLAY").is_none()
        && let Some(d) = desktop_socket()
    {
        c.env("WAYLAND_DISPLAY", d);
    }
    let child = c
        .args(args)
        .env("LC_ALL", "C")
        .env("SYSTEMD_PAGER", "")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(TIMEOUT, child).await.ok()?.ok()?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() && s.trim().is_empty() {
        s = String::from_utf8_lossy(&out.stderr).into_owned();
    }
    Some(strip_colors(&s))
}

/// Removes terminal colour codes (ESC [ … letter).
fn strip_colors(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'[') {
            it.next();
            for d in it.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The desktop's Wayland socket in this person's runtime folder, if one is running.
fn desktop_socket() -> Option<String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("wayland-") && !n.ends_with(".lock"))
        .collect();
    names.sort();
    names.into_iter().next()
}

async fn cmd(r: &mut String, title: &str, prog: &str, args: &[&str]) {
    let body = run(prog, args).await.unwrap_or_else(|| format!("({prog} is not available)"));
    block(r, title, &body);
}

async fn journal(r: &mut String, title: &str, args: &[&str], keep: usize) {
    let mut a = vec!["--no-pager", "-q"];
    a.extend_from_slice(args);
    let out = run("journalctl", &a).await.unwrap_or_else(|| "(the journal could not be read)".into());
    let lines: Vec<&str> = out.lines().collect();
    let body = if lines.len() > keep {
        format!("  ({} earlier lines left out)\n{}", lines.len() - keep, lines[lines.len() - keep..].join("\n"))
    } else {
        out.clone()
    };
    block(r, title, &body);
}

fn uptime(secs: &str) -> String {
    let s = secs.parse::<f64>().unwrap_or(0.0) as u64;
    format!("{} h {} min", s / 3600, s % 3600 / 60)
}

fn boot_mode() -> String {
    if !Path::new("/sys/firmware/efi").is_dir() {
        return "BIOS (legacy)".into();
    }
    // SecureBoot-<guid>: 4 bytes of attributes, then 1 = on.
    let sb = std::fs::read_dir("/sys/firmware/efi/efivars")
        .ok()
        .and_then(|d| {
            d.flatten()
                .find(|e| e.file_name().to_string_lossy().starts_with("SecureBoot-"))
                .and_then(|e| std::fs::read(e.path()).ok())
        })
        .and_then(|b| b.get(4).copied());
    match sb {
        Some(1) => "UEFI, Secure Boot on".into(),
        Some(_) => "UEFI, Secure Boot off".into(),
        None => "UEFI".into(),
    }
}

fn link_name(p: &Path) -> String {
    std::fs::read_link(p).ok().and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default()
}

fn sys(p: &Path, f: &str) -> String {
    std::fs::read_to_string(p.join(f)).unwrap_or_default().trim().to_string()
}

fn sorted_dir(p: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(p).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// Graphics cards with their kernel driver, and each screen connector.
fn graphics(r: &mut String) {
    let mut cards = String::new();
    let mut screens = String::new();
    for p in sorted_dir("/sys/class/drm") {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(rest) = name.strip_prefix("card") else { continue };
        if !rest.contains('-') {
            let dev = p.join("device");
            let _ = writeln!(
                cards,
                "  {name}: driver {} (pci {}:{}) {}",
                or_none(&link_name(&dev.join("driver"))),
                sys(&dev, "vendor").trim_start_matches("0x"),
                sys(&dev, "device").trim_start_matches("0x"),
                if sys(&dev, "boot_vga") == "1" { "· used at start-up" } else { "" }
            );
            continue;
        }
        let status = sys(&p, "status");
        let modes = sys(&p, "modes");
        let mut l = format!("  {name}: {status}");
        if status == "connected" {
            if let Some(best) = modes.lines().next() {
                let _ = write!(l, ", {best}");
            }
            if let Some((w, h)) = std::fs::read(p.join("edid")).ok().as_deref().and_then(edid_size_cm) {
                let _ = write!(l, ", {w}×{h} cm");
                if let Some(dpi) = modes.lines().next().and_then(|m| dpi(m, w)) {
                    let _ = write!(l, ", about {dpi} dpi");
                }
            }
        }
        let _ = writeln!(screens, "{l}");
    }
    block(r, "Graphics cards", if cards.is_empty() { "  (none: no graphics driver loaded)" } else { &cards });
    block(r, "Screen connectors", &screens);
}

fn or_none(s: &str) -> &str {
    if s.is_empty() { "none" } else { s }
}

/// Physical size from an EDID block (bytes 21 and 22, in cm; 0 = unknown).
fn edid_size_cm(e: &[u8]) -> Option<(u32, u32)> {
    let (w, h) = (*e.get(21)? as u32, *e.get(22)? as u32);
    (w > 0 && h > 0).then_some((w, h))
}

/// Dots per inch from a mode such as "2560x1600" and the width in cm.
fn dpi(mode: &str, width_cm: u32) -> Option<u32> {
    let px: u32 = mode.split('x').next()?.parse().ok()?;
    Some((px as f64 / (width_cm as f64 / 2.54)).round() as u32)
}

/// Network adapters, whether each is wireless, and its driver.
fn net(r: &mut String) {
    let mut s = String::new();
    for p in sorted_dir("/sys/class/net") {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name == "lo" || !p.join("device").exists() {
            continue;
        }
        let kind = if p.join("wireless").exists() || p.join("phy80211").exists() { "Wi-Fi" } else { "wired" };
        let _ = writeln!(
            s,
            "  {name}: {kind}, driver {}, {}",
            or_none(&link_name(&p.join("device/driver"))),
            sys(&p, "operstate")
        );
    }
    block(r, "Network adapters", if s.is_empty() { "  (none found)" } else { &s });
}

/// Batteries and chargers.
fn power(r: &mut String) {
    let mut s = String::new();
    for p in sorted_dir("/sys/class/power_supply") {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match sys(&p, "type").as_str() {
            "Battery" => {
                let full = sys(&p, "energy_full").parse::<f64>().or_else(|_| sys(&p, "charge_full").parse());
                let design =
                    sys(&p, "energy_full_design").parse::<f64>().or_else(|_| sys(&p, "charge_full_design").parse());
                let health = match (full, design) {
                    (Ok(f), Ok(d)) if d > 0.0 => format!(", holds {:.0}% of its original charge", f / d * 100.0),
                    _ => String::new(),
                };
                let _ = writeln!(
                    s,
                    "  {name}: battery {}%, {}{health}",
                    sys(&p, "capacity"),
                    sys(&p, "status").to_lowercase()
                );
            }
            "Mains" => {
                let on = if sys(&p, "online") == "1" { "plugged in" } else { "not plugged in" };
                let _ = writeln!(s, "  {name}: charger {on}");
            }
            _ => {}
        }
    }
    block(r, "Battery", if s.is_empty() { "  (no battery: a desktop computer)" } else { &s });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edid_size() {
        let mut e = vec![0u8; 128];
        e[21] = 34;
        e[22] = 19;
        assert_eq!(edid_size_cm(&e), Some((34, 19)));
        e[21] = 0;
        assert_eq!(edid_size_cm(&e), None);
        assert_eq!(edid_size_cm(&[0; 10]), None);
    }

    #[test]
    fn dots_per_inch() {
        // A 13.3" 2560x1600 laptop panel is about 29 cm wide: ~224 dpi.
        assert_eq!(dpi("2560x1600", 29), Some(224));
        assert_eq!(dpi("1920x1080", 53), Some(92));
        assert_eq!(dpi("nonsense", 30), None);
    }

    #[test]
    fn clipping_keeps_the_end() {
        let s: Vec<String> = (0..MAX_LINES + 5).map(|i| i.to_string()).collect();
        let c = clip(&s.join("\n"));
        assert!(c.starts_with("  (5 earlier lines left out)"));
        assert!(c.ends_with(&(MAX_LINES + 4).to_string()));
    }

    #[test]
    fn colours_are_removed() {
        assert_eq!(strip_colors("\x1b[1mVirtual-1\x1b[0m \x1b[1;32m(enabled)\x1b[0m"), "Virtual-1 (enabled)");
    }

    #[test]
    fn uptime_reads() {
        assert_eq!(uptime("3725.4"), "1 h 2 min");
    }

    #[tokio::test]
    async fn report_has_every_section() {
        let text = collect().await;
        for s in
            ["## System", "## Graphics", "## Network", "## Sound", "## Power and sleep", "## Devices", "## Problems"]
        {
            assert!(text.contains(s), "missing {s}");
        }
    }
}

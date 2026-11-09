//! Applying `/etc/iliada`: `nixos-rebuild switch`, with its output streamed.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Run `program args…`, passing each output line (stdout and stderr) to
/// `log`. On failure the error carries the last lines of output.
pub async fn run_logged(program: &str, args: &[&str], log: &(dyn Fn(String) + Sync)) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("running {program}"))?;
    let mut out = BufReader::new(child.stdout.take().expect("piped")).lines();
    let mut err = BufReader::new(child.stderr.take().expect("piped")).lines();
    let mut tail: VecDeque<String> = VecDeque::new();
    let (mut out_done, mut err_done) = (false, false);
    while !(out_done && err_done) {
        let line = tokio::select! {
            l = out.next_line(), if !out_done => l?.or_else(|| { out_done = true; None }),
            l = err.next_line(), if !err_done => l?.or_else(|| { err_done = true; None }),
        };
        if let Some(line) = line {
            if tail.len() == 12 {
                tail.pop_front();
            }
            tail.push_back(line.clone());
            log(line);
        }
    }
    let status = child.wait().await?;
    if !status.success() {
        // Nix puts the useful part after the first "error:".
        let text: Vec<String> = tail.into();
        let start = text.iter().position(|l| l.contains("error:")).unwrap_or(0);
        bail!("{program} failed:\n{}", text[start..].join("\n"));
    }
    Ok(())
}

/// Build and switch to the configuration in `dir`.
pub async fn rebuild(dir: &Path, log: &(dyn Fn(String) + Sync)) -> Result<()> {
    let flake = format!("git+file://{}#iliada", dir.display());
    run_logged("nixos-rebuild", &["switch", "--flake", &flake], log).await
}

/// Is `name` a package in the system's nixpkgs?
pub async fn package_exists(dir: &Path, name: &str) -> bool {
    let attr = format!("git+file://{}#nixosConfigurations.iliada.pkgs.{name}.name", dir.display());
    Command::new("nix")
        .args(["--extra-experimental-features", "nix-command flakes", "eval", "--raw", &attr])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

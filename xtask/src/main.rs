//! Repository automation: `cargo xtask <command>`.
//!
//!   ci          fmt check, clippy (deny warnings), tests — what CI runs
//!   linux-test  run the test suite inside a Linux container (Landlock, real paths)
//!   demo-ingest ingest a package (default: debian:jq) into ./out
//!   demo-broker start the broker for examples/bundles/voice-memo.app

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_owned();
    std::env::set_current_dir(&root)?;
    match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("linux-test") => linux_test(&root),
        Some("demo-ingest") => run(
            "cargo",
            &[
                "run",
                "-p",
                "iliada-ingest",
                "--",
                "run",
                args.get(1).map(String::as_str).unwrap_or("debian:jq"),
                "--out",
                "out",
                "--force",
            ],
        ),
        Some("demo-broker") => {
            let dir = std::env::temp_dir().join("iliada-demo");
            println!("sockets in {}/sock — in another terminal run:", dir.display());
            println!(
                "  cargo run -p iliada-guest --example request-mic -- {}/sock/org.iliada.voice-memo.vsock_7000",
                dir.display()
            );
            run(
                "cargo",
                &[
                    "run",
                    "-p",
                    "iliada-broker",
                    "--",
                    "--bundle",
                    "examples/bundles/voice-memo.app",
                    "--socket-dir",
                    &format!("{}/sock", dir.display()),
                    "--state-dir",
                    &format!("{}/state", dir.display()),
                ],
            )
        }
        _ => {
            eprintln!("usage: cargo xtask <ci|linux-test|demo-ingest [source]|demo-broker>");
            std::process::exit(2);
        }
    }
}

fn ci() -> Result<()> {
    run("cargo", &["fmt", "--all", "--check"])?;
    run("cargo", &["clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"])?;
    run("cargo", &["test", "--workspace", "--all-features"])
}

/// Prefer Docker (OrbStack/Docker Desktop); fall back to a Lima VM named "iliada".
/// Build output and the crate cache go under Cargo's configured target dir
/// (which may be on another disk), never into the source tree.
fn linux_test(root: &std::path::Path) -> Result<()> {
    let target = target_dir()?;
    if have("docker") {
        let src = format!("{}:/src", root.display());
        let tgt = format!("{}:/target", target.join("linux").display());
        let reg = format!("{}:/usr/local/cargo/registry", target.join("linux-cargo-registry").display());
        return run(
            "docker",
            &[
                "run",
                "--rm",
                "-v",
                &src,
                "-v",
                &tgt,
                "-v",
                &reg,
                "-w",
                "/src",
                "rust:1",
                "bash",
                "-c",
                "cargo test --workspace --all-features --target-dir /target",
            ],
        );
    }
    if have("limactl") {
        let script = format!("cargo test --workspace --all-features --target-dir {}/linux", target.display());
        return run("limactl", &["shell", "--workdir", &root.display().to_string(), "iliada", "bash", "-lc", &script]);
    }
    bail!("need docker or limactl for linux-test")
}

/// Cargo's effective target directory (honours `build.target-dir` and `CARGO_TARGET_DIR`).
fn target_dir() -> Result<PathBuf> {
    let out = Command::new("cargo").args(["metadata", "--format-version", "1", "--no-deps"]).output()?;
    let text = String::from_utf8(out.stdout)?;
    let key = "\"target_directory\":\"";
    let start = text
        .find(key)
        .map(|i| i + key.len())
        .ok_or_else(|| anyhow::anyhow!("no target_directory in cargo metadata"))?;
    let end = text[start..].find('"').map(|i| start + i).ok_or_else(|| anyhow::anyhow!("malformed cargo metadata"))?;
    Ok(PathBuf::from(&text[start..end]))
}

fn have(bin: &str) -> bool {
    Command::new(bin).arg("--version").output().is_ok_and(|o| o.status.success())
}

fn run(cmd: &str, args: &[&str]) -> Result<()> {
    eprintln!("$ {cmd} {}", args.join(" "));
    let status = Command::new(cmd).args(args).status()?;
    if !status.success() {
        bail!("{cmd} failed with {status}");
    }
    Ok(())
}

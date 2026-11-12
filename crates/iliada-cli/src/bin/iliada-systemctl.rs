//! `systemctl` as a person types it on Iliada.
//!
//! System services are part of the computer's configuration (/etc/iliada),
//! so the real `systemctl enable`/`disable` can't change them (the unit
//! links are read-only). For the services Iliada knows (SSH, printing,
//! nginx, PostgreSQL, MariaDB, Redis, Docker), this turns
//! `systemctl enable sshd` into the same settings change as
//! `iliada install ssh`: written to settings.toml, recorded in its history,
//! backed up, applied now and after every restart. `start`/`stop` stay quick
//! and temporary, and say so. Everything else, and anything run by a script
//! (not at a terminal), goes to the real systemctl unchanged.
//!
//! Usage: iliada-systemctl <path of the real systemctl> [systemctl arguments…]

use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

use iliada_system::client;
use iliada_system::protocol::{Event, Request};
use iliada_system::settings::{SERVICES, service_of_unit};

#[derive(Debug, PartialEq)]
enum Plan {
    /// Hand everything to the real systemctl.
    PassThrough,
    /// `enable`/`disable` of Iliada services: a settings change.
    Setting { services: Vec<&'static str>, on: bool },
    /// `start`/`stop`/`restart` of Iliada services: run it, then explain.
    Runtime { verb: String, services: Vec<&'static str> },
    /// `enable`/`disable` of other system units: run it, explain if it fails.
    OtherUnits,
}

/// Options that take a value as the next argument.
const WITH_VALUE: &[&str] = &["-t", "--type", "-p", "--property", "-s", "--signal", "-n", "--lines", "-o", "--output"];
/// Options that mean "not this computer's system services".
const ELSEWHERE: &[&str] = &["--user", "--global", "-H", "--host", "-M", "--machine", "--root", "--image"];

fn plan(args: &[String]) -> Plan {
    let mut words = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        let name = a.split('=').next().unwrap_or(a);
        if ELSEWHERE.contains(&name) {
            return Plan::PassThrough;
        }
        if WITH_VALUE.contains(&a.as_str()) {
            skip = true;
        } else if !a.starts_with('-') {
            words.push(a.as_str());
        }
    }
    let Some((verb, units)) = words.split_first() else { return Plan::PassThrough };
    if units.is_empty() {
        return Plan::PassThrough;
    }
    let services: Option<Vec<&'static str>> = units.iter().map(|u| service_of_unit(u)).collect();
    match (*verb, services) {
        ("enable" | "disable", Some(mut s)) => {
            s.dedup();
            Plan::Setting { services: s, on: *verb == "enable" }
        }
        ("enable" | "disable" | "mask" | "unmask", None) => Plan::OtherUnits,
        ("start" | "stop" | "restart" | "try-restart" | "reload-or-restart", Some(s)) => {
            Plan::Runtime { verb: (*verb).to_owned(), services: s }
        }
        _ => Plan::PassThrough,
    }
}

fn describe(service: &str) -> &'static str {
    SERVICES.iter().find(|(n, _)| *n == service).map(|(_, d)| *d).unwrap_or("this service")
}

fn run(real: &str, args: &[String]) -> i32 {
    Command::new(real).args(args).status().map(|s| s.code().unwrap_or(1)).unwrap_or(1)
}

async fn set(services: &[&'static str], on: bool) -> anyhow::Result<()> {
    for s in services {
        let what = describe(s);
        eprintln!(
            "{} is one of this computer's settings: turning it {} in /etc/iliada/settings.toml \
             (kept after restarts, recorded and backed up)…",
            what,
            if on { "on" } else { "off" }
        );
        let mut on_event = |e: &Event| {
            if let Event::Log { line } = e {
                let short: String = line.chars().take(100).collect();
                eprint!("\r\x1b[2K  {short}");
            }
        };
        let r = client::call(&Request::SetService { name: (*s).to_owned(), on }, &mut on_event).await;
        eprint!("\r\x1b[2K");
        let r = r?;
        let changed = r.get("changed").and_then(|c| c.as_bool()).unwrap_or(true);
        let state = if on { "on" } else { "off" };
        eprintln!("{what} {} {state}.", if changed { "is" } else { "was already" });
    }
    Ok(())
}

async fn configured_on(service: &str) -> Option<bool> {
    let st = client::status().await.ok()?;
    Some(st.services.iter().any(|x| x.name == service && x.on))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut argv = std::env::args().skip(1);
    let Some(real) = argv.next() else {
        eprintln!("usage: iliada-systemctl <systemctl> [arguments…]");
        return ExitCode::from(2);
    };
    let args: Vec<String> = argv.collect();
    // Scripts get the real systemctl, exactly.
    let person = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let plan = if person { plan(&args) } else { Plan::PassThrough };
    match plan {
        Plan::PassThrough => {
            let e = Command::new(&real).args(&args).exec();
            eprintln!("systemctl: {e}");
            ExitCode::from(1)
        }
        Plan::Setting { services, on } => match set(&services, on).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e:#}");
                ExitCode::from(1)
            }
        },
        Plan::Runtime { verb, services } => {
            let code = run(&real, &args);
            for s in services {
                let id = s;
                match (verb.as_str(), code == 0, configured_on(s).await) {
                    ("stop", true, Some(true)) => eprintln!(
                        "Stopped until the next restart: {} is on in this computer's settings. \
                         To turn it off for good: systemctl disable {} (or iliada remove {id})",
                        describe(s),
                        iliada_system::settings::unit_of_service(s).unwrap_or(s),
                    ),
                    (_, false, Some(false)) => eprintln!(
                        "{} is off in this computer's settings. To turn it on (kept after restarts): \
                         systemctl enable {} (or iliada install {id})",
                        describe(s),
                        iliada_system::settings::unit_of_service(s).unwrap_or(s),
                    ),
                    _ => {}
                }
            }
            ExitCode::from(code.clamp(0, 255) as u8)
        }
        Plan::OtherUnits => {
            let code = run(&real, &args);
            if code != 0 {
                eprintln!(
                    "System services are part of this computer's configuration: add them to \
                     /etc/iliada/configuration.nix and run `iliada system apply`. Your own services \
                     work as usual: systemctl --user enable …"
                );
            }
            ExitCode::from(code.clamp(0, 255) as u8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Plan {
        plan(&s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
    }

    #[test]
    fn enabling_iliada_services_is_a_setting() {
        assert_eq!(p("enable sshd"), Plan::Setting { services: vec!["ssh"], on: true });
        assert_eq!(p("enable --now sshd.service"), Plan::Setting { services: vec!["ssh"], on: true });
        assert_eq!(p("disable nginx mysql"), Plan::Setting { services: vec!["nginx", "mariadb"], on: false });
        assert_eq!(p("-q disable docker.service docker.socket"), Plan::Setting { services: vec!["docker"], on: false });
    }

    #[test]
    fn start_and_stop_stay_temporary() {
        assert_eq!(p("stop sshd"), Plan::Runtime { verb: "stop".into(), services: vec!["ssh"] });
        assert_eq!(p("restart cups"), Plan::Runtime { verb: "restart".into(), services: vec!["printing"] });
    }

    #[test]
    fn everything_else_is_untouched() {
        assert_eq!(p("status sshd"), Plan::PassThrough);
        assert_eq!(p("--user enable syncthing"), Plan::PassThrough);
        assert_eq!(p("enable --user sshd"), Plan::PassThrough);
        assert_eq!(p("list-units"), Plan::PassThrough);
        assert_eq!(p(""), Plan::PassThrough);
        assert_eq!(p("daemon-reload"), Plan::PassThrough);
        assert_eq!(p("enable NetworkManager-wait-online"), Plan::OtherUnits);
        assert_eq!(p("enable sshd NetworkManager"), Plan::OtherUnits);
        assert_eq!(p("start NetworkManager"), Plan::PassThrough);
        assert_eq!(p("-t service list-units"), Plan::PassThrough);
    }
}

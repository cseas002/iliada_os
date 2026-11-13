//! `systemctl` as a person types it on Iliada.
//!
//! System services are part of the computer's configuration (/etc/iliada),
//! so the real `systemctl enable`/`disable` can't change them (the unit
//! links are read-only). For the services Iliada knows (SSH, printing,
//! nginx, PostgreSQL, MariaDB, Redis, Docker) the three levels work as on
//! Debian or Fedora:
//!
//! * `iliada install ssh` / `iliada remove ssh`: on the computer or not;
//! * `systemctl enable sshd` / `disable sshd`: started at every boot or not
//!   (saved in settings.toml, recorded, backed up; enabling a service that
//!   isn't installed installs it; `--now` also starts or stops it);
//! * `systemctl start sshd` / `stop sshd`: running now or not, until the
//!   next restart.
//!
//! Everything else, and anything run by a script (not at a terminal), goes
//! to the real systemctl unchanged.
//!
//! Usage: iliada-systemctl <path of the real systemctl> [systemctl arguments…]

use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

use iliada_system::client;
use iliada_system::protocol::{Event, Request, ServiceInfo};
use iliada_system::settings::{SERVICES, service_of_unit, unit_of_service};

#[derive(Debug, PartialEq)]
enum Plan {
    /// Hand everything to the real systemctl.
    PassThrough,
    /// `enable`/`disable` of Iliada services: started at boot or not (a
    /// setting); `now`: also start or stop them.
    Boot { services: Vec<&'static str>, boot: bool, now: bool },
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
    let (mut skip, mut now) = (false, false);
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        let name = a.split('=').next().unwrap_or(a);
        if ELSEWHERE.contains(&name) {
            return Plan::PassThrough;
        }
        if a == "--now" {
            now = true;
        } else if WITH_VALUE.contains(&a.as_str()) {
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
            Plan::Boot { services: s, boot: *verb == "enable", now }
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

fn unit(service: &str) -> &str {
    unit_of_service(service).unwrap_or(service)
}

fn run(real: &str, args: &[String]) -> i32 {
    Command::new(real).args(args).status().map(|s| s.code().unwrap_or(1)).unwrap_or(1)
}

async fn state(service: &str) -> Option<ServiceInfo> {
    client::status().await.ok()?.services.into_iter().find(|s| s.name == service)
}

async fn set_boot(real: &str, services: &[&'static str], boot: bool, now: bool) -> anyhow::Result<()> {
    for s in services {
        let what = describe(s);
        let before = state(s).await;
        let installing = boot && !before.as_ref().is_some_and(|b| b.on);
        eprintln!(
            "{what}: {} in /etc/iliada/settings.toml (kept, recorded and backed up)…",
            match (boot, installing) {
                (true, true) => "installing it and starting it at every boot",
                (true, false) => "starting it at every boot",
                (false, _) => "not starting it at boot",
            }
        );
        let mut on_event = |e: &Event| {
            if let Event::Log { line } = e {
                let short: String = line.chars().take(100).collect();
                eprint!("\r\x1b[2K  {short}");
            }
        };
        let r = client::call(&Request::SetServiceBoot { name: (*s).to_owned(), boot }, &mut on_event).await;
        eprint!("\r\x1b[2K");
        r?;
        match (boot, before.map(|b| b.on)) {
            (true, _) => eprintln!("{what} starts at every boot."),
            (false, Some(true)) if !now => eprintln!(
                "{what} won't start at boot any more. It keeps running until you stop it \
                 (systemctl stop {}) or restart; to remove it: iliada remove {s}",
                unit(s)
            ),
            (false, Some(true)) => eprintln!("{what} won't start at boot any more."),
            (false, _) => eprintln!("{what} isn't installed (iliada install {s})."),
        }
        if now {
            let verb = if boot { "start" } else { "stop" };
            run(real, &[verb.into(), unit(s).into()]);
        }
    }
    Ok(())
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
        Plan::Boot { services, boot, now } => match set_boot(&real, &services, boot, now).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e:#}");
                ExitCode::from(1)
            }
        },
        Plan::Runtime { verb, services } => {
            let code = run(&real, &args);
            for s in services {
                let info = state(s).await;
                match (verb.as_str(), code == 0, info) {
                    ("stop", true, Some(i)) if i.boot => eprintln!(
                        "Stopped until the next restart: {} starts at every boot. To change that: \
                         systemctl disable {} (or remove it: iliada remove {s})",
                        describe(s),
                        unit(s),
                    ),
                    (_, false, Some(i)) if !i.on => eprintln!(
                        "{} isn't installed. Install it (it then starts at every boot): iliada install {s}",
                        describe(s),
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
    fn enable_and_disable_are_start_at_boot() {
        assert_eq!(p("enable sshd"), Plan::Boot { services: vec!["ssh"], boot: true, now: false });
        assert_eq!(p("enable --now sshd.service"), Plan::Boot { services: vec!["ssh"], boot: true, now: true });
        assert_eq!(
            p("disable nginx mysql"),
            Plan::Boot { services: vec!["nginx", "mariadb"], boot: false, now: false }
        );
        assert_eq!(
            p("-q disable --now docker.service docker.socket"),
            Plan::Boot { services: vec!["docker"], boot: false, now: true }
        );
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

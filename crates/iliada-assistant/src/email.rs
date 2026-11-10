//! Sending email from the user's own account (SMTP), for the assistant's
//! `send_email` tool.
//!
//! The account is the user's: `~/.config/iliada/email.toml` (address,
//! server) and `~/.config/iliada/email-password`, both readable only by
//! them and never backed up. Set in Settings → Assistant → Email, or with
//! `iliada assistant email`.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use lettre::message::{Mailbox, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::{Deserialize, Serialize};

/// How the connection to the mail server is protected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// Encrypted from the start (usually port 465).
    #[default]
    Tls,
    /// Plain connection upgraded with STARTTLS, which is required (usually 587).
    Starttls,
    /// No encryption: only for a mail server on this computer (testing).
    None,
}

impl Security {
    pub fn default_port(self) -> u16 {
        match self {
            Security::Tls => 465,
            Security::Starttls => 587,
            Security::None => 25,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailAccount {
    /// The address mail is sent from.
    pub address: String,
    /// The name shown with it ("Ada Lovelace").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The outgoing (SMTP) server.
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default)]
    pub security: Security,
    /// The sign-in name, if it isn't the address.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
}

impl EmailAccount {
    pub fn port(&self) -> u16 {
        self.port.unwrap_or_else(|| self.security.default_port())
    }

    pub fn username(&self) -> &str {
        if self.username.is_empty() { &self.address } else { &self.username }
    }

    /// The name shown with the address: the one set for the account, else
    /// the person's full name on this computer.
    pub fn display_name(&self) -> Option<String> {
        if !self.name.trim().is_empty() {
            return Some(self.name.trim().to_owned());
        }
        full_name()
    }

    fn mailbox(&self) -> Result<Mailbox> {
        let address = self.address.parse().with_context(|| format!("{:?} is not an email address", self.address))?;
        Ok(Mailbox::new(self.display_name(), address))
    }

    pub fn validate(&self) -> Result<()> {
        self.mailbox()?;
        ensure!(!self.server.trim().is_empty(), "the outgoing mail server is missing");
        if self.security == Security::None {
            ensure!(
                is_local(&self.server),
                "an unencrypted connection is only allowed to a mail server on this computer"
            );
        }
        Ok(())
    }

    /// The usual settings for well-known providers, from the address.
    pub fn suggested(address: &str) -> Option<EmailAccount> {
        let domain = address.rsplit_once('@')?.1.to_lowercase();
        let (server, security) = match domain.as_str() {
            "gmail.com" | "googlemail.com" => ("smtp.gmail.com", Security::Tls),
            "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => ("smtp-mail.outlook.com", Security::Starttls),
            "icloud.com" | "me.com" | "mac.com" => ("smtp.mail.me.com", Security::Starttls),
            "yahoo.com" | "ymail.com" => ("smtp.mail.yahoo.com", Security::Tls),
            "fastmail.com" | "fastmail.fm" => ("smtp.fastmail.com", Security::Tls),
            "gmx.com" | "gmx.net" | "gmx.de" => ("mail.gmx.net", Security::Tls),
            "zoho.com" => ("smtp.zoho.com", Security::Tls),
            _ => return None,
        };
        Some(EmailAccount { address: address.into(), server: server.into(), security, ..Default::default() })
    }
}

/// The user's full name from the account database (`Ada Lovelace,,,`).
fn full_name() -> Option<String> {
    let user = std::env::var("USER").ok()?;
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    let gecos = passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 4 && f[0] == user).then(|| f[4].split(',').next().unwrap_or("").trim().to_owned())
    })?;
    (!gecos.is_empty()).then_some(gecos)
}

fn is_local(host: &str) -> bool {
    matches!(host.trim(), "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

/// One email.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Email {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
}

fn config_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    Some(cfg.join("iliada"))
}

/// Where the account and its password are kept.
#[derive(Debug, Clone)]
pub struct Files {
    pub account: PathBuf,
    pub password: PathBuf,
}

impl Files {
    /// `~/.config/iliada/email.toml` and `email-password`.
    pub fn user() -> Option<Files> {
        let dir = config_dir()?;
        Some(Files { account: dir.join("email.toml"), password: dir.join("email-password") })
    }

    pub fn load(&self) -> Option<EmailAccount> {
        toml::from_str(&std::fs::read_to_string(&self.account).ok()?).ok()
    }

    pub fn password(&self) -> Option<String> {
        let p = std::fs::read_to_string(&self.password).ok()?;
        let p = p.trim_end_matches(['\n', '\r']);
        (!p.is_empty()).then(|| p.to_owned())
    }

    /// Save the account; `password: None` keeps the saved one.
    pub fn save(&self, account: &EmailAccount, password: Option<&str>) -> Result<()> {
        account.validate()?;
        write_private(&self.account, toml::to_string(account)?.as_bytes())?;
        if let Some(p) = password {
            write_private(&self.password, p.as_bytes())?;
        }
        Ok(())
    }

    /// Forget the account and its password.
    pub fn remove(&self) -> Result<()> {
        for p in [&self.account, &self.password] {
            match std::fs::remove_file(p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(())
    }
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    std::fs::create_dir_all(path.parent().context("no parent folder")?)?;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(bytes)?;
    Ok(())
}

/// The user's account, if one is set up.
pub fn load() -> Option<EmailAccount> {
    Files::user()?.load()
}

pub fn has_password() -> bool {
    Files::user().and_then(|f| f.password()).is_some()
}

/// Save the user's account; `password: None` keeps the saved one.
pub fn save(account: &EmailAccount, password: Option<&str>) -> Result<()> {
    Files::user().context("HOME is not set")?.save(account, password)
}

/// Forget the user's account and its password.
pub fn remove() -> Result<()> {
    Files::user().context("HOME is not set")?.remove()
}

fn build(account: &EmailAccount, email: &Email) -> Result<Message> {
    ensure!(!email.to.is_empty(), "no recipient");
    let mut m = Message::builder().from(account.mailbox()?).subject(email.subject.clone());
    for to in &email.to {
        m = m.to(to.parse().with_context(|| format!("{to:?} is not an email address"))?);
    }
    for cc in &email.cc {
        m = m.cc(cc.parse().with_context(|| format!("{cc:?} is not an email address"))?);
    }
    Ok(m.header(ContentType::TEXT_PLAIN).body(email.body.clone())?)
}

/// Send `email` from the saved account.
pub async fn send(email: &Email) -> Result<()> {
    let account = load().context("no email account is set up yet (Settings → Assistant → Email)")?;
    send_with(&account, Files::user().and_then(|f| f.password()).as_deref(), email).await
}

/// Send `email` from `account` (with its password, if it needs one).
pub async fn send_with(account: &EmailAccount, password: Option<&str>, email: &Email) -> Result<()> {
    account.validate()?;
    let message = build(account, email)?;
    let host = account.server.trim();
    let mut t = match account.security {
        Security::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(host)?,
        Security::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)?,
        Security::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host).tls(Tls::None),
    }
    .port(account.port())
    .timeout(Some(std::time::Duration::from_secs(60)));
    if account.security == Security::Starttls {
        t = t.tls(Tls::Required(TlsParameters::new(host.into())?));
    }
    if let Some(p) = password {
        t = t.credentials(Credentials::new(account.username().to_owned(), p.to_owned()));
    } else if account.security != Security::None {
        bail!("the email password is missing (Settings → Assistant → Email)");
    }
    t.build().send(message).await.context("the mail server did not accept the email")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// A tiny SMTP server on 127.0.0.1 that accepts one message and returns
    /// what it received (the commands and the message).
    async fn sink() -> (u16, tokio::task::JoinHandle<String>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(async move {
            let (c, _) = l.accept().await.unwrap();
            let (r, mut w) = c.into_split();
            let mut r = BufReader::new(r);
            let mut log = String::new();
            w.write_all(b"220 sink ESMTP\r\n").await.unwrap();
            let mut data = false;
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                log.push_str(&line);
                if data {
                    if line == ".\r\n" {
                        data = false;
                        w.write_all(b"250 queued\r\n").await.unwrap();
                    }
                    continue;
                }
                let cmd = line.to_uppercase();
                let reply: &[u8] = if cmd.starts_with("EHLO") {
                    b"250-sink\r\n250 AUTH PLAIN LOGIN\r\n"
                } else if cmd.starts_with("AUTH") {
                    b"235 ok\r\n"
                } else if cmd.starts_with("DATA") {
                    data = true;
                    b"354 go\r\n"
                } else if cmd.starts_with("QUIT") {
                    w.write_all(b"221 bye\r\n").await.unwrap();
                    break;
                } else {
                    b"250 ok\r\n"
                };
                w.write_all(reply).await.unwrap();
            }
            log
        });
        (port, h)
    }

    #[tokio::test]
    async fn sends_through_a_local_server() {
        let (port, server) = sink().await;
        let account = EmailAccount {
            address: "ada@example.org".into(),
            name: "Ada".into(),
            server: "127.0.0.1".into(),
            port: Some(port),
            security: Security::None,
            username: String::new(),
        };
        let email = Email {
            to: vec!["bob@example.org".into()],
            cc: vec![],
            subject: "Lunch".into(),
            body: "See you at noon.".into(),
        };
        send_with(&account, None, &email).await.unwrap();
        let log = server.await.unwrap();
        assert!(log.contains("MAIL FROM:<ada@example.org>"), "{log}");
        assert!(log.contains("RCPT TO:<bob@example.org>"), "{log}");
        assert!(log.contains("Subject: Lunch"), "{log}");
        assert!(log.contains("See you at noon."), "{log}");
    }

    #[test]
    fn no_unencrypted_mail_to_other_computers() {
        let mut a = EmailAccount {
            address: "ada@example.org".into(),
            server: "smtp.example.org".into(),
            security: Security::None,
            ..Default::default()
        };
        assert!(a.validate().is_err());
        a.security = Security::Starttls;
        a.validate().unwrap();
        assert_eq!(a.port(), 587);
        assert_eq!(a.username(), "ada@example.org");
        assert!(EmailAccount { address: "not an address".into(), ..a.clone() }.validate().is_err());
    }

    #[test]
    fn knows_common_providers() {
        let g = EmailAccount::suggested("someone@Gmail.com").unwrap();
        assert_eq!((g.server.as_str(), g.security, g.port()), ("smtp.gmail.com", Security::Tls, 465));
        assert!(EmailAccount::suggested("me@my-own-domain.example").is_none());
    }

    #[test]
    fn the_account_round_trips_privately() {
        let d = tempfile::tempdir().unwrap();
        let files =
            Files { account: d.path().join("iliada/email.toml"), password: d.path().join("iliada/email-password") };
        let a = EmailAccount::suggested("ada@gmail.com").unwrap();
        files.save(&a, Some("app password")).unwrap();
        assert_eq!(files.load().unwrap(), a);
        assert_eq!(files.password().unwrap(), "app password");
        use std::os::unix::fs::PermissionsExt as _;
        for p in [&files.account, &files.password] {
            assert_eq!(std::fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        files.save(&a, None).unwrap();
        assert_eq!(files.password().unwrap(), "app password", "kept");
        files.remove().unwrap();
        assert!(files.load().is_none() && files.password().is_none());
    }
}

//! `iliada account`: the Iliada account on this computer (also in Settings →
//! Account). Each person signs in for themselves; no administrator needed.

use std::io::{IsTerminal, Write};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use iliada_account::{self as account, Files, KeyStep, Status};

#[derive(Subcommand)]
pub enum AccountCmd {
    /// Signed in or not, and the last backup.
    Status,
    /// Sign in: you approve a code on the account website, with your passkey
    /// or an emailed code.
    SignIn {
        /// The account server (default: the one Iliada was set up with).
        #[arg(long)]
        server: Option<String>,
    },
    /// Show your recovery key again.
    RecoveryKey,
    /// Type your recovery key (a computer needs it once to read your backup).
    EnterKey,
    /// Get your account key from another computer already signed in (instead
    /// of typing the recovery key): it shows a code to compare there.
    GetKey,
    /// Hand your account key to a new computer waiting for it, after checking
    /// it shows the same code.
    GiveKey,
    /// Back up your settings now.
    Backup {
        /// Do nothing (and say nothing) if this computer isn't signed in;
        /// how the hourly backup runs.
        #[arg(long)]
        if_signed_in: bool,
    },
    /// Put your account's backup in place on this computer.
    Restore,
    /// Sign this computer out (your account and backup stay).
    SignOut,
}

pub async fn run(cmd: AccountCmd) -> Result<()> {
    let files = Files::user()?;
    match cmd {
        AccountCmd::Status => match account::status(&files) {
            Status::SignedOut => {
                println!("Not signed in. Sign in with: iliada account sign-in");
                if account::default_server().is_none() {
                    println!("(This computer has no Iliada account server set up yet; use --server.)");
                }
            }
            Status::NeedsKey { email } => {
                println!(
                    "Signed in as {email}, but this computer needs your account key: iliada account get-key\n(from another computer signed in) or iliada account enter-key (your recovery key)"
                )
            }
            Status::Ready { email, last_backup, last_error } => {
                println!("Signed in as {email}.");
                let when = last_backup.map(crate::ago).unwrap_or_else(|| "not yet".into());
                println!("Last backup: {when}");
                if let Some(e) = last_error {
                    println!("The last backup failed: {e}");
                }
                println!("Backed up, encrypted on this computer:");
                for (_, what) in account::PARTS {
                    println!("  {what}");
                }
            }
        },
        AccountCmd::SignIn { server } => sign_in(&files, server).await?,
        AccountCmd::RecoveryKey => match account::recovery_key(&files) {
            Some(k) => println!("{k}"),
            None => bail!("this computer doesn't have your account key"),
        },
        AccountCmd::EnterKey => enter_key(&files).await?,
        AccountCmd::GetKey => get_key(&files).await?,
        AccountCmd::GiveKey => give_key(&files).await?,
        AccountCmd::Backup { if_signed_in } => {
            if if_signed_in && !matches!(account::status(&files), Status::Ready { .. }) {
                return Ok(());
            }
            match account::backup(&files, !if_signed_in).await? {
                account::Backup::Uploaded { files, bytes } => {
                    eprintln!("Backed up {files} files ({} KB, encrypted).", bytes.div_ceil(1024))
                }
                account::Backup::Unchanged => eprintln!("Nothing changed since the last backup."),
            }
        }
        AccountCmd::Restore => {
            let (m, n) = account::restore(&files).await?;
            eprintln!("Restored {n} files from {} (backed up {}).", m.computer, crate::ago(m.created));
        }
        AccountCmd::SignOut => {
            account::sign_out(&files).await?;
            eprintln!("Signed out. Your account and its backup are still there.");
        }
    }
    Ok(())
}

async fn sign_in(files: &Files, server: Option<String>) -> Result<()> {
    let server = server
        .or_else(account::default_server)
        .context("this computer has no Iliada account server set up; give one with --server")?;
    let code = account::start_sign_in(&server).await?;
    eprintln!("To sign in this computer, open\n\n    {}\n", code.page());
    eprintln!("sign in there, and approve the code  {}\n", code.user_code);
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some() {
        let _ = std::process::Command::new("xdg-open")
            .arg(code.page())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    eprintln!("Waiting for the approval… (Ctrl+C stops)");
    let (email, step) = account::finish_sign_in(files, &server, &code).await?;
    eprintln!("\nSigned in as {email}.");
    match step {
        KeyStep::Created(key) => {
            eprintln!("\nYour recovery key:\n\n    {key}\n");
            eprintln!(
                "Write it down or print it, and keep it somewhere safe. Your backup is encrypted on this\n\
                 computer with it, so nobody else can read it, Iliada included. A new computer needs it\n\
                 once; if you lose every computer and this key, the backup can't be read.\n\
                 Show it again with: iliada account recovery-key"
            );
        }
        KeyStep::Needed => {
            eprintln!("\nYour account's backup is encrypted. This computer needs your account key once:");
            eprintln!("from a computer already signed in (iliada account get-key), or your recovery key.");
            if std::io::stdin().is_terminal() {
                enter_key(files).await?;
            } else {
                eprintln!("Type it with: iliada account enter-key");
            }
        }
        KeyStep::Ready => {}
    }
    offer_restore(files).await;
    Ok(())
}

async fn enter_key(files: &Files) -> Result<()> {
    loop {
        eprint!("Recovery key: ");
        std::io::stderr().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 || line.trim().is_empty() {
            bail!("no recovery key typed; do it later with: iliada account enter-key");
        }
        match account::use_recovery_key(files, &line).await {
            Ok(()) => {
                eprintln!("That's it: this computer can read your backup now.");
                return Ok(());
            }
            Err(e) if std::io::stdin().is_terminal() => eprintln!("{e:#} (Press Enter to stop.)"),
            Err(e) => return Err(e),
        }
    }
}

async fn offer_restore(files: &Files) {
    if let Ok(Some(m)) = account::latest(files).await
        && m.computer != account::computer_name()
    {
        eprintln!(
            "\nYour account has a backup from {} ({}). Put it on this computer with: iliada account restore",
            m.computer,
            crate::ago(m.created)
        );
    }
}

async fn get_key(files: &Files) -> Result<()> {
    let request = account::ask_for_key(files).await?;
    eprintln!(
        "On a computer already signed in to your account, open Settings → Iliada account\n\
         (or run: iliada account give-key) and approve this computer if it shows the code\n\n    {}\n",
        request.code
    );
    eprintln!("Waiting… (Ctrl+C stops)");
    account::wait_for_key(files, &request).await?;
    eprintln!("That's it: this computer can read your backup now.");
    offer_restore(files).await;
    Ok(())
}

async fn give_key(files: &Files) -> Result<()> {
    if !matches!(account::status(files), Status::Ready { .. }) {
        bail!("this computer doesn't have your account key, so it can't hand it over");
    }
    let waiting = account::waiting_for_key(files).await?;
    if waiting.is_empty() {
        eprintln!("No computer is waiting for your account key. On the new one: iliada account get-key");
        return Ok(());
    }
    for w in &waiting {
        eprintln!("\"{}\" asks for your account key. It should show the code  {}", w.computer, w.code);
        if !std::io::stdin().is_terminal() {
            bail!("run this in a terminal to answer");
        }
        eprint!("Same code on that computer? Hand it the key? [y/N] ");
        std::io::stderr().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim().eq_ignore_ascii_case("y") || line.trim().eq_ignore_ascii_case("yes") {
            account::hand_over_key(files, w).await?;
            eprintln!("Done: \"{}\" has your account key.", w.computer);
        } else {
            account::refuse_key(files, w).await?;
            eprintln!("Refused.");
        }
    }
    Ok(())
}

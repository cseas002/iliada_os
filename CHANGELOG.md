# Changelog

## 0.2.0 (in progress)

**No terminal, no sudo.**
- **Iliada system service** (`iliada-systemd`): the one thing that changes
  the system. Administrators (`wheel`) use it through Settings, the Store,
  the assistant and the `iliada` command; nothing needs `sudo`.
- **Iliada Settings**: Updates, Backup, Command-line tools, Services,
  Privacy, Assistant, About.
- **Setup Assistant** on first login (replaces COSMIC's wizard): restore a
  backup, or start backing up.
- **Graphical installer** ("Install Iliada") on the live system.
- **GitHub backup and restore**: `/etc/iliada` pushed to a private
  `iliada_<name>` repository after every change; restoring brings back apps
  and settings on a new computer.
- **Iliada updates**: signed release manifests; Settings → Updates.

**Apps, software, services.**
- The Store has **Software** (Python, Node.js, ffmpeg…) and **Services**
  (nginx, PostgreSQL, MariaDB, Redis, Docker, SSH, printing), labeled as not
  apps; they install through the system service.
- New apps: Sudoku (replaces 2048), VLC; LibreOffice Writer in progress.
- The package translator keeps libraries in a program's own folder where
  the program expects them.

**Privacy and isolation.**
- **Permission prompts**: apps ask on first launch for the folders, camera
  and microphone they declare; answers in Settings → Privacy. Files opened
  with an app are given to it one by one.
- **Micro-VMs**: Settings → Privacy → Extra isolation runs an app in its own
  virtual machine (its own kernel; window through waypipe; software
  rendering; no network or sound yet).

**The assistant.**
- Ask Iliada in plain words (Assistant app, `iliada ask`). **Ask** mode
  approves every action; **Auto** mode acts on its own with a trusted model
  (Claude Opus 5.5 by default). Only Iliada's own operations; every action
  logged. Configured in `settings.toml`; your API key stays on your computer.

**For testers and developers.**
- Ready-to-run VM images (UTM on Apple Silicon, QEMU) that set up
  `/etc/iliada` on first boot.
- `utm-dev` image and `scripts/dev-push.sh`: update a running test VM in
  place, keeping its setup.

## 0.1.0

First developer preview: installer ISO, COSMIC desktop, `.app` bundles in
`/Applications` (drag in to install, Trash to remove), the Iliada Store with
12 apps, sandboxing, settings.toml, the `iliada` command.

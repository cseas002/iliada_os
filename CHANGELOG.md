# Changelog

## 0.2.1 (2026-11-12)

- The assistant's answers appear as they are written instead of all at once
  (both Claude and OpenRouter models).
- Frosted glass by default: windows, the Alt+Space panel, the top panel and
  the dock are see-through with the desktop blurred behind them (COSMIC
  Settings → Appearance turns it off, and it stays off).
- Models are shown by name ("Claude Opus 5.5", "DeepSeek V4 Flash") in the panel
  and in Settings → Assistant; the panel no longer shows a model while the
  assistant is off.
- Settings: a message such as "Iliada is up to date" no longer stays on screen
  after switching to another page.
- `iliada install` installs anything in the Store: an app (`iliada install
  firefox`), software (`iliada install python`, also by everyday names such as
  node, rust, postgres) or a service (`iliada install ssh` turns it on).
  `iliada remove` undoes it; `iliada search <word>` finds things.
- The Store's search box searches apps, software and services together, from
  any page.
- The assistant understands everyday names when installing software ("python").
- `iliada assistant key < key.txt` reads an API key from a file (a `.env` file
  works too), for computers where pasting is not possible.
- The right-click menu (Cut, Copy, Paste) of a text field on a scrolled page
  opens where you clicked, not at the bottom of the screen (a libcosmic fix).
- New guide: [docs/using.md](docs/using.md), how to install apps, Python and
  services, update, back up and use the assistant.
- Permission prompts say "The app is able to use this but didn't say why" when the app
  gave no reason (they showed a technical note about libraries).

## 0.2.0 (2026-11-11)

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
- **Iliada updates**: signed release manifests; Settings → Updates. Tested end to end
  with a privately served release.

**Apps, software, services.**
- **Apps for ARM computers** (Apple Silicon in UTM, Raspberry Pi): the Store
  offers the build for your computer's processor (14 apps on ARM so far),
  and an app made for another kind of computer says so instead of failing
  silently. Iliada 0.1 keeps reading the old catalog.
- Apps start for every user (the sandbox group was missing on installed
  systems and the UTM image, so apps failed there).
- The Store has **Software** (Python, Node.js, ffmpeg…) and **Services**
  (nginx, PostgreSQL, MariaDB, Redis, Docker, SSH, printing), labeled as not
  apps; they install through the system service.
- New apps in the Store: LibreOffice Writer, VLC, Sudoku (replaces 2048).
  LibreOffice Writer works (it had picked up Debian's headless core).
- Recipes can declare folders, permissions and `copy` steps (what a Debian
  install script would have done; Iliada never runs those).
- Apps see a one-user `/etc/passwd` (theirs) and a home with the usual folders.
- Bundles that declare permissions from a newer Iliada still open (unknown
  permissions are skipped). Apps republished with `sound` need 0.2.
- The package translator keeps libraries in a program's own folder where
  the program expects them.

**Privacy and isolation.**
- **Permission prompts**: apps ask on first launch for the folders, camera
  and microphone they declare; answers in Settings → Privacy. Files opened
  with an app are given to it one by one.
- **Micro-VMs**: Settings → Privacy → Extra isolation runs an app in its own
  virtual machine (its own kernel; window through waypipe; software
  rendering). Apps that declare a network get one through passt (no root;
  nothing forwarded in; the computer's own local services stay out of
  reach); sound goes over vsock to your sound server (tested with Firefox). Inside the VM the app
  runs as you, so browsers' own sandboxes work. Files you open with such an app reach
  it one by one, like outside a VM.

**The assistant.**
- Ask Iliada in plain words: press **Alt+Space** anywhere for the "Ask
  Iliada…" panel (or open the Assistant app, or `iliada ask`). **Ask** mode
  approves every action; **Auto** mode acts on its own. Claude (Opus 5.5 by
  default) or any OpenRouter model, such as DeepSeek V4 Flash at a fraction
  of a cent per request; what it has cost is shown in Settings. Every action
  logged. Configured in `settings.toml`; your API key stays on your computer.
- It switches dark/light appearance, sets the volume, opens links, and with
  **full access** (your choice) runs commands as you.
- **Email** from your own account (Settings → Assistant → Email). You see
  every email before it goes, even in Auto mode.
- Text in files and command output is never taken as instructions; it tells
  you when a file tried.

**For testers and developers.**
- Everyday command-line tools on every system: lspci, lsusb, sensors, dig,
  traceroute, ifconfig, lsof, killall, jq, vim, git, tmux, ripgrep, tldr and
  more (`iliada.baseTools`), next to lscpu, ip and the other basics.
- The Iliada logo: a Greek helmet with a horsehair crest, shown by
  `fastfetch` (and `neofetch`).
- Ready-to-run VM images (UTM on Apple Silicon, QEMU) that set up
  `/etc/iliada` on first boot.
- `utm-dev` image and `scripts/dev-push.sh`: update a running test VM in
  place, keeping its setup.

## 0.1.0

First developer preview: installer ISO, COSMIC desktop, `.app` bundles in
`/Applications` (drag in to install, Trash to remove), the Iliada Store with
12 apps, sandboxing, settings.toml, the `iliada` command.

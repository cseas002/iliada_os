# Changelog

## Unreleased

- **A new computer gets your account key from one already signed in**: no
  need to type the recovery key. On the new computer, Settings → Iliada
  account → "Ask another computer" shows a code; on a computer already
  signed in, Settings → Iliada account shows the request with the same code
  and Approve. The key travels sealed for that one computer (the account
  server only passes it on and can't read it), and the matching codes catch
  anyone in the middle. Also `iliada account get-key` / `give-key`.

## 0.2.6 (2026-11-20)

- **One Settings window.** Iliada's pages (the Iliada account, Privacy,
  Homer, Updates, Backup, Services, Command-line tools, About Iliada) are
  now pages of COSMIC Settings, next to Wi-Fi, Displays and Sound: one app
  called Settings, one sidebar. (Before, Displays and the like opened a
  second settings window.)
- **Google Chrome**: ticked as the browser in the Setup Assistant, in the
  Store (downloaded from Google and set up on the computer), and the
  browser you choose opens links and sits in the dock.
- **Chrome had no internet**: apps built on the computer (Chrome, Debian
  apps added later) now get the network when they're a browser, mail or
  chat app.
- **Iliad names and icons for the built-in apps**: Files, Text Editor,
  Terminal, Media Player (a lyre), Photos, System Monitor, Document Reader
  (a scroll), Screenshot, and the dock's search, workspaces and app
  library, all in the bronze-and-navy style; "COSMIC" is gone from their
  names. GVim, Vim and the advanced network editor leave the app library.

- **Homer asks before important things done in your name** (sending a
  message or an email) even in Auto mode, unless you let him: Settings →
  Homer → "Send messages and emails without asking", or just tell him.
  Changing that always needs your yes, so nothing he reads (an email, a web
  page) can give him more freedom.
- "Checked 1 hour ago", not "1 hours ago" (Settings, Homer and the
  `iliada` command now share one wording).
- A smaller USB image: the live session leaves out COSMIC's document
  reader (OCR for every language, 1 GB) and System76's scheduler (kernel
  tracing tools); installed systems keep both.
- **Safe against power cuts**: the configuration's history is now written
  to disk before a change counts as done, and if a crash still damages it,
  Iliada mends it on its own before the next change or update. (A test
  computer switched off right after an update could not update again.)

## 0.2.5 (2026-11-19)

- **The Iliada account is online** at account.iliada-os.com: Settings →
  Account and the Setup Assistant offer it on every Iliada. Sign in with
  an email code, then a passkey; desktop settings, Privacy answers and
  conversations with Homer are backed up encrypted, readable only by you.

- **The Iliad look**: a new default wallpaper, Homer's wine-dark sea at
  dusk with Troy on its hill and the Achaean ships, and a bronze-on-navy
  theme (deep bronze on parchment in light mode) for windows, the panel,
  the dock and the lock screen. People who picked their own colours in
  Appearance keep them.
- New icons for Settings (a bronze gear with a meander), the Setup
  Assistant (a laurel wreath around an alpha) and the installer (a Greek
  ship), to match the Store and Homer.

## 0.2.4 (2026-11-19)

- **Choose your apps** in the Setup Assistant: a browser (Firefox, Google
  Chrome or Chromium) and everyday apps (LibreOffice, VLC, photos, PDFs…),
  with a few ticked to start. They install in the background after setup
  and a notification says when they are ready. Skipped after a restore,
  which brings its own apps. `iliada install --keep-going` goes on when one
  name fails.
- **Homer**: the assistant has a name (the poet of the Iliad, and he knows
  it well) and an icon, a bronze coin with his profile. The Store has its
  own icon too: a stoa of the agora.
- **One Settings app**: "Iliada Settings" is now just **Settings**, and its
  sidebar lists Wi-Fi, Bluetooth, Displays, Sound, Appearance, Keyboard,
  Power, Users and COSMIC's other pages next to Iliada's own (each opens
  right at that page). COSMIC Settings leaves Applications and the dock;
  Homer joins the dock.
- Google Chrome (and other apps that keep their logo under /opt) shows its
  own icon instead of a gear.
- The project moved to the **iliada-os** organization on GitHub; updates,
  the Store catalog and links use the new addresses (the old ones redirect).

## 0.2.3 (2026-11-18)

- **Updates download instead of compiling**: Iliada's own programs come
  ready-made from cache.iliada-os.com (signed), so an update takes minutes
  and works on computers with little memory.
- **"Iliada 0.2.3 is available"**: a notice after start-up when an update is
  ready (once per version); clicking it opens Iliada Settings → Updates.
  Iliada now looks for updates 2 minutes after start-up and every 6 hours
  (it was an hour after start-up, then daily, so a computer switched off
  sooner never looked).
- The installer: install **next to another system** (ChromeOS Flex,
  Windows…) in its free space, the time zone found from the internet
  connection and chosen from region and city lists, and a page that scrolls
  instead of cutting off the password fields. `iliada report` lists the
  disks.
- **Google Chrome**: `iliada install chrome` downloads it from Google and
  makes it into a sandboxed app on the computer (Chrome's licence keeps it
  out of the Store).
- **Test updates**: Settings → Updates → Test updates (or
  `iliada system testing on`) gets Iliada's newest work as soon as it is
  ready, between releases. Fixes arrive sooner, and so do new problems;
  every update can still be undone from the start-up menu. Test updates
  are named after their time ("0.2.2, test update of 18 Nov 2026 15:30
  UTC"), and the next release replaces them.
- **The Iliada account** (first version, for computers set up with an account
  server): Settings → Account and a new Setup Assistant step sign in with a
  code you approve on the account website. Your desktop settings, Privacy
  answers and assistant conversations are backed up every hour, encrypted on
  the computer first with a key only your computers have (you keep a
  recovery key for a new computer). Restore brings them back on another
  computer at once. Also `iliada account`. Iliada's own account server isn't
  online yet.
- New in the Store (no Iliada update needed): Clocks, File Roller (open and
  make zip archives) and Okular (documents).
- Okular, GIMP, Thunderbird and VLC no longer ask for the microphone when
  they first open; they never record. Installed copies update to the fixed
  version automatically.
- **Passwords: Bitwarden comes with Firefox and Chromium**, pinned to the
  toolbar, ready to log in or create a free account. The browsers' own
  password saving is off, so passwords are kept in one place (and Bitwarden
  also keeps passkeys). Turn it off in the browser's extensions page.
- Backups sign in through Iliada's own GitHub app, so they work without
  any setup. Only Iliada's backup repositories are listed for restore
  (other repositories whose names start with `iliada_` were listed too), and
  a backup is never written into a repository that isn't one.
- Permission questions stay above every window (they could end up hidden
  behind another one, so an app looked as if it never opened).
- fastfetch and the system's version file say "Iliada 0.2.2", not the NixOS
  release it is built on.

## 0.2.2 (2026-11-14)

- New in the Store: LibreOffice Calc, Impress and Math, added by the catalog
  builder, which now finds popular Debian apps, builds them, launch-tests
  them and publishes the ones that start (`iliada_apps/scripts/grow.sh`).
- Apps built from Debian bring what a desktop has anyway: GTK apps get the
  SVG image loader (game tiles and icons were blank), Qt apps Qt's Wayland
  plugin.
- **Apps that aren't in the Store**: search for them anyway. Debian's ~2,000
  desktop apps are listed under "Not in the Store yet" with a **Build**
  button; Iliada builds the app from Debian's package on this computer into
  an ordinary sandboxed `.app`, and says plainly when one can't be built. The
  same with `iliada install krita` (asks first) and the assistant ("install
  Krita").
- **The Assistant is an app**: your conversations in the sidebar, each saved
  and continued where you left it (even after switching models), and
  **Activity**: everything the assistant did, with **Undo** for what can be
  reversed (an app installed, a service turned on, dark or light), and every
  change made to this computer from anywhere. An undo is noted in its
  conversation, and the assistant is told. Alt+Space stays a quick panel; its
  conversations are saved too, and "Open in Assistant" continues one in the app.
- The assistant can answer "what changed on this computer?", and knows the
  current appearance and volume without guessing.
- Settings → Assistant: the API key shows whether one is saved
  ("OpenRouter key saved (sk-or-v1…36b3)"), checks it with the provider (free;
  OpenRouter also says how much credit is left), and has Replace and Remove.
  **Add key** is enabled only for a key that fits the chosen model, with a
  line saying what is wrong otherwise.
- Services have three levels, as on Debian or Fedora: **installed**
  (`iliada install ssh`, the Store's Turn on), **starts at boot**
  (`systemctl enable sshd` / `disable`, Settings → Services → Start
  automatically) and **running now** (`systemctl start` / `stop`, until the
  next restart, and it says so). The first two are kept in settings.toml
  (`ssh = "manual"`: installed, started only when asked), recorded and backed
  up. Typed in a terminal, `systemctl` works this way for sshd, cups, nginx,
  postgresql, mysql, redis and docker; scripts get the ordinary systemctl.
- `iliada service list` shows whether each service is installed, starts at boot and is running
  now; `iliada history` lists every change to this computer.
- **Hardware report**: Settings → About → **Save report** (or `iliada report`)
  saves a text file on the Desktop describing this computer: graphics driver,
  screens and how sharp they are, Wi-Fi, sound, sleep, battery, missing
  firmware and errors. Share it with whoever helps when something doesn't work.
- Laptops: power modes (Balanced, Power saver, Performance) and battery
  levels; processor fixes from Intel and AMD are loaded at start-up.
- Proposal: your own Iliada server for encrypted backups of the computer and
  of each person's settings ([docs/self-hosted-sync.md](docs/self-hosted-sync.md)).

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

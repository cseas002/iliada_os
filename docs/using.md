# Using Iliada: how do I…

Everything below can be done without a terminal, in the **Store** (in the
dock) or **Iliada Settings**. If you like the terminal, the `iliada` command
does the same things; you never need `sudo`.

There are three kinds of things you can add:

| | What it is | Example | Where |
|---|---|---|---|
| **App** | A program with a window. Lives in Applications as one `.app`, runs in its own sandbox. | Firefox, GIMP, VLC | Store → Discover |
| **Software** | A program you use in the Terminal. Installed for everyone on the computer. | Python, Node.js, ffmpeg, Git | Store → Software |
| **Service** | A program that runs in the background, even when nobody is logged in. | SSH server, nginx, PostgreSQL | Store → Services |

The `iliada install` command works out which kind you mean.

## Install an app

Open the **Store**, find the app, click **Get**. It appears in Applications
and the launcher within seconds. The first time it starts, it asks for what it
needs (a folder, the camera, the microphone).

```sh
iliada install firefox
iliada install libreoffice-writer      # names with spaces: use dashes
```

To remove an app, drag it from Applications to the Trash (or
`iliada remove firefox`).

## Install Python (or Node.js, Go, ffmpeg…)

Store → **Software** → Python → **Get**. Or type "python" in the Store's
search box from any page. It takes a few minutes the first time (the system
is rebuilt with Python in it), then open a new Terminal:

```sh
iliada install python        # also: node, go, rust, gcc, ffmpeg, git, vim…
python3 --version
```

Several at once: `iliada install python node git`. Remove with
`iliada remove python`.

**Something that is not in the Store?** Any program packaged in nixpkgs
(search at [search.nixos.org/packages](https://search.nixos.org/packages))
can be installed by its package name:

```sh
iliada tool add httpie
```

**Python libraries:** use a virtual environment in your project, as
everywhere else:

```sh
python3 -m venv .venv && . .venv/bin/activate && pip install requests
```

## Turn on a service (SSH, a web server, a database)

Store → **Services** → **Turn on**, or Settings → Services. From the
terminal:

```sh
iliada install ssh           # same as: iliada service enable ssh
iliada install postgres
iliada remove ssh            # turns it off
```

## Find something

Type in the Store's search box: it searches apps, software and services
together. Or:

```sh
iliada search database
iliada list                  # everything in the Store, and what you have
```

## Update

Apps update on their own. Iliada itself: Settings → **Updates** →
**Update now** (or `iliada system update`), then restart. If an update ever
goes wrong, pick the previous entry in the boot menu.

## Back up, and move to a new computer

Settings → **Backup** → Sign in with GitHub → name the backup. Your apps,
software, services and settings are saved to a private GitHub repository
after every change. On the new computer, the Setup Assistant (first login)
or Settings → Backup → **Restore** brings them back. Your own files
(Documents, Pictures…) are not in this backup.

## Ask the assistant

Press **Alt+Space** anywhere and type, for example "install VLC and turn on
dark mode". First turn it on in Settings → **Assistant** and add an API key:
Anthropic (Claude) or OpenRouter (many models, some very cheap). In **Ask me
first** mode, you approve every action.

From the terminal: `iliada ask "turn on ssh"`, or set the key with
`iliada assistant key` (it reads the key from the keyboard, or from a file:
`iliada assistant key < key.txt`).

## Who can change what

Anyone can install and remove their own apps. Software, services, updates
and backups change the whole computer, so they need an **administrator**
(the first account is one). Others see the buttons greyed out.

## For testers: getting files into a virtual machine

Copy and paste between your computer and a VM shown over VNC often does not
work. Copy files over SSH instead:

* **The dev VM** (`run-iliada-dev-vm`) listens for SSH on port 2222 of the
  computer running it (user `iliada`, password `iliada`):
  ```sh
  scp -P 2222 key.txt iliada@127.0.0.1:
  ```
  From another computer, hop through the one running the VM:
  ```sh
  scp -J you@vm-host -P 2222 key.txt iliada@127.0.0.1:
  ```
* **An installed Iliada**: turn on Settings → Services → SSH server first,
  then `scp key.txt you@<its address>:`.

Then, in the VM, `iliada assistant key < key.txt && rm key.txt` sets an API
key without typing it.

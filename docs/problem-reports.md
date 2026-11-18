# Sending problem reports to Iliada (design, for later)

Status: **idea, not built.** Today `iliada report` (Settings → About →
Save report) writes a text file the person can read and share by hand.

## What a person gets

* After something goes wrong (an app crashes, an update fails, the screen or
  Wi-Fi misbehaves), Iliada offers: **"Send a report to Iliada?"** with the
  report shown in full first. Nothing is ever sent without that click.
* Settings → Privacy → **Problem reports**: *Ask each time* (the default),
  *Never*, or *Always send* (still never with personal files, see below).
* Each report can be **anonymous** (the default) or **with a way to reach
  them**: their Iliada account, or an email address, so the developers can
  ask a question or say it is fixed.
* They can see what they sent, and its state ("fixed in 0.2.4"), in
  Settings → Privacy → Problem reports.

## What a report holds

What `iliada report` already collects (hardware, drivers, screens, network
adapters, sound, sleep, disks, failed services, recent errors), minus
anything identifying: the computer's name, user names, home folder paths,
IP and MAC addresses and serial numbers are replaced before it leaves the
computer (`<computer>`, `<user>`, …). Never files, browser history,
passwords, assistant conversations or app data.

## How it reaches the developers

1. The computer sends the report to the Iliada account server
   (`account.iliada-os.com`, the server in iliada_cloud), over HTTPS, signed
   in or not. Rate-limited per computer.
2. The server groups reports that look the same (same app and error, same
   hardware problem) so one problem is one issue, with a count.
3. A new group becomes a **GitHub issue** in iliada-os/iliada (labels
   `report`, `hardware`/`app`/`update`), through a GitHub App that can only
   create and comment on issues. Later reports of the same problem add a
   comment ("+3 computers, HP EliteDesk 800 G2") instead of new issues.
4. Reports with a way to reach the person keep that contact on the server
   only; the public issue never shows it.

## Why this way

* The person sees and decides first: no silent telemetry.
* Issues are public, so the anonymising step runs on the computer, before
  sending, and is tested with real reports.
* One server for accounts and reports, both already in Rust (iliada_cloud).

## To build it

1. Anonymising in `iliada report` (`--for-sending`), with tests on saved
   real reports.
2. `POST /v1/reports` on the account server: store, group, rate-limit.
3. The GitHub App and the issue/comment step (a small worker on the server).
4. The prompts in Settings and after a crash or failed update.

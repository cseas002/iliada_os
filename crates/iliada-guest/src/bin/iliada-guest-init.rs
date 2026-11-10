//! `iliada-guest-init`: PID 1 of an app's micro-VM.
//!
//! Mounts the basics, loads the virtio drivers, reads `spec.json` from the
//! `spec` share, brings up the network and the sound relay if the spec
//! has them, mounts the
//! shares it lists (at their host paths) and runs the program it names:
//! waypipe, carrying the app's window to the host, around the same
//! bubblewrap sandbox as Tier 1. When the program exits, the VM powers off.

#[cfg(target_os = "linux")]
fn main() {
    let code = match linux::run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("iliada-guest-init: {e:#}");
            1
        }
    };
    eprintln!("iliada-guest-init: the app exited ({code}); powering off");
    linux::power_off();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("iliada-guest-init runs only inside a Linux micro-VM");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;
    use std::process::Command;

    use anyhow::{Context, Result};
    use iliada_ipc::guest::{GuestNetwork, GuestSpec, SOUND_VSOCK_PORT, SPEC_DIR, SPEC_TAG};
    use rustix::mount::{MountFlags, UnmountFlags, mount, mount_move, unmount};
    use rustix::process::{chdir, chroot};

    fn mount_fs(source: &str, target: &str, fs: &str, flags: MountFlags) -> Result<()> {
        std::fs::create_dir_all(target)?;
        // virtio devices appear asynchronously after their drivers load.
        let mut tries = 0;
        loop {
            match mount(source, target, fs, flags, None) {
                Ok(()) => return Ok(()),
                Err(e) if fs == "virtiofs" && tries < 50 => {
                    let _ = e;
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => return Err(e).with_context(|| format!("mounting {fs} {source} on {target}")),
            }
        }
    }

    pub fn run() -> Result<i32> {
        let nosuid = MountFlags::NOSUID | MountFlags::NODEV;
        mount_fs("proc", "/proc", "proc", nosuid | MountFlags::NOEXEC)?;
        mount_fs("sysfs", "/sys", "sysfs", nosuid | MountFlags::NOEXEC)?;
        mount_fs("devtmpfs", "/dev", "devtmpfs", MountFlags::NOSUID)?;

        // Drivers for the shares and the host channel (modules in the initrd).
        let status = Command::new("/bin/modprobe")
            .args(["-a", "virtio_pci", "virtiofs", "vmw_vsock_virtio_transport", "virtio_rng", "virtio_net"])
            .status()
            .context("running modprobe")?;
        if !status.success() {
            eprintln!("iliada-guest-init: some drivers did not load");
        }

        // What to run. Read now, while `ip` (in the initramfs) is at hand.
        mount_fs(SPEC_TAG, SPEC_DIR, "virtiofs", MountFlags::RDONLY | nosuid)?;
        let spec: GuestSpec =
            serde_json::from_slice(&std::fs::read(Path::new(SPEC_DIR).join("spec.json")).context("reading spec.json")?)
                .context("parsing spec.json")?;
        unmount(SPEC_DIR, UnmountFlags::empty()).context("unmounting the spec")?;
        ip(&["link", "set", "lo", "up"])?;
        if let Some(net) = &spec.network {
            network(net)?;
        }

        // Leave the initramfs for a tmpfs root, as switch_root does: the
        // sandbox (bubblewrap) needs pivot_root, which rootfs refuses.
        mount_fs("tmpfs", "/sysroot", "tmpfs", MountFlags::empty())?;
        for d in ["proc", "sys", "dev"] {
            let to = format!("/sysroot/{d}");
            std::fs::create_dir_all(&to)?;
            mount_move(format!("/{d}"), &to).with_context(|| format!("moving /{d}"))?;
        }
        chdir("/sysroot")?;
        mount_move(".", "/").context("moving the new root")?;
        chroot(".").context("entering the new root")?;
        chdir("/")?;

        mount_fs("tmpfs", "/run", "tmpfs", nosuid)?;
        mount_fs("tmpfs", "/tmp", "tmpfs", nosuid)?;
        std::fs::create_dir_all("/dev/pts")?;
        let _ = mount("devpts", "/dev/pts", "devpts", MountFlags::NOSUID | MountFlags::NOEXEC, None);
        let _ = mount("tmpfs", "/dev/shm", "tmpfs", nosuid, None).or_else(|_| {
            std::fs::create_dir_all("/dev/shm")?;
            mount("tmpfs", "/dev/shm", "tmpfs", nosuid, None).map_err(std::io::Error::from)
        });

        etc(&spec)?;
        for share in &spec.shares {
            let mut flags = MountFlags::NODEV;
            if share.readonly {
                flags |= MountFlags::RDONLY;
            }
            mount_fs(&share.tag, &share.path, "virtiofs", flags)?;
        }
        let _ = std::fs::write("/proc/sys/kernel/hostname", "iliada-app");
        if let Some((_, dir)) = spec.env.iter().find(|(k, _)| k == "XDG_RUNTIME_DIR") {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }

        if let Some(socket) = &spec.sound {
            sound_relay(socket)?;
        }

        let status = Command::new(&spec.program)
            .args(&spec.args)
            .env_clear()
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .status()
            .with_context(|| format!("starting {}", spec.program))?;
        Ok(status.code().unwrap_or(1))
    }

    fn ip(args: &[&str]) -> Result<()> {
        let status = Command::new("/bin/ip").args(args).status().context("running ip")?;
        anyhow::ensure!(status.success(), "ip {} failed", args.join(" "));
        Ok(())
    }

    /// The first network card (virtio-net), once its driver has bound.
    fn card() -> Result<String> {
        for _ in 0..50 {
            if let Ok(dir) = std::fs::read_dir("/sys/class/net") {
                let mut names: Vec<String> =
                    dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n != "lo").collect();
                names.sort();
                if let Some(n) = names.into_iter().next() {
                    return Ok(n);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        anyhow::bail!("the VM's network card did not appear")
    }

    /// Static addresses (passt answers for the gateway and DNS); IPv6, if
    /// the host has it, configures itself from passt's router adverts.
    fn network(net: &GuestNetwork) -> Result<()> {
        let card = card()?;
        ip(&["link", "set", &card, "up"])?;
        ip(&["addr", "add", &net.address, "dev", &card])?;
        ip(&["route", "add", "default", "via", &net.gateway, "dev", &card])?;
        Ok(())
    }

    /// `/etc` in the guest's root: what the sandbox binds from it.
    fn etc(spec: &GuestSpec) -> Result<()> {
        std::fs::create_dir_all("/etc")?;
        std::fs::write("/etc/hosts", "127.0.0.1 localhost iliada-app\n::1 localhost iliada-app\n")?;
        if let Some(net) = &spec.network {
            std::fs::write("/etc/resolv.conf", format!("nameserver {}\n", net.dns))?;
        }
        for (path, target) in &spec.links {
            if let Some(parent) = Path::new(path).parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::os::unix::fs::symlink(target, path).with_context(|| format!("linking {path}"))?;
        }
        Ok(())
    }

    /// Listen where the sandbox expects the sound server and carry each
    /// connection to the host over vsock, where `iliada-run` hands it to
    /// the user's sound server.
    fn sound_relay(socket: &str) -> Result<()> {
        use std::os::unix::net::UnixListener;
        if let Some(dir) = Path::new(socket).parent() {
            std::fs::create_dir_all(dir)?;
        }
        let listener = UnixListener::bind(socket).with_context(|| format!("listening on {socket}"))?;
        std::thread::spawn(move || {
            for app in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let host = match vsock::VsockStream::connect_with_cid_port(vsock::VMADDR_CID_HOST, SOUND_VSOCK_PORT)
                    {
                        Ok(h) => h,
                        Err(e) => return eprintln!("iliada-guest-init: sound: {e}"),
                    };
                    let (Ok(mut app_r), Ok(mut host_w)) = (app.try_clone(), host.try_clone()) else { return };
                    let up = std::thread::spawn(move || {
                        let _ = std::io::copy(&mut app_r, &mut host_w);
                        let _ = host_w.shutdown(std::net::Shutdown::Write);
                    });
                    let (mut app_w, mut host_r) = (app, host);
                    let _ = std::io::copy(&mut host_r, &mut app_w);
                    let _ = app_w.shutdown(std::net::Shutdown::Write);
                    let _ = up.join();
                });
            }
        });
        Ok(())
    }

    pub fn power_off() -> ! {
        rustix::fs::sync();
        let _ = rustix::system::reboot(rustix::system::RebootCommand::PowerOff);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
}

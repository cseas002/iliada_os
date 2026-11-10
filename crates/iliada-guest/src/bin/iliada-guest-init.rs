//! `iliada-guest-init`: PID 1 of an app's micro-VM.
//!
//! Mounts the basics, loads the virtio-fs and vsock drivers, reads
//! `spec.json` from the `spec` share, mounts the shares it lists (at their
//! host paths) and runs the program it names: waypipe, carrying the app's
//! window to the host, around the same bubblewrap sandbox as Tier 1. When
//! the program exits, the VM powers off.

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
    use iliada_ipc::guest::{GuestSpec, SPEC_DIR, SPEC_TAG};
    use rustix::mount::{MountFlags, mount};

    fn mount_fs(source: &str, target: &str, fs: &str, flags: MountFlags) -> Result<()> {
        std::fs::create_dir_all(target)?;
        mount(source, target, fs, flags, None).with_context(|| format!("mounting {fs} {source} on {target}"))
    }

    pub fn run() -> Result<i32> {
        let nosuid = MountFlags::NOSUID | MountFlags::NODEV;
        mount_fs("proc", "/proc", "proc", nosuid | MountFlags::NOEXEC)?;
        mount_fs("sysfs", "/sys", "sysfs", nosuid | MountFlags::NOEXEC)?;
        mount_fs("devtmpfs", "/dev", "devtmpfs", MountFlags::NOSUID)?;
        mount_fs("tmpfs", "/run", "tmpfs", nosuid)?;
        mount_fs("tmpfs", "/tmp", "tmpfs", nosuid)?;
        std::fs::create_dir_all("/dev/pts")?;
        let _ = mount("devpts", "/dev/pts", "devpts", MountFlags::NOSUID | MountFlags::NOEXEC, None);
        let _ = mount("tmpfs", "/dev/shm", "tmpfs", nosuid, None).or_else(|_| {
            std::fs::create_dir_all("/dev/shm")?;
            mount("tmpfs", "/dev/shm", "tmpfs", nosuid, None).map_err(std::io::Error::from)
        });

        // Drivers for the shares and the host channel (modules in the initrd).
        let status = Command::new("/bin/modprobe")
            .args(["-a", "virtiofs", "vmw_vsock_virtio_transport", "virtio_rng"])
            .status()
            .context("running modprobe")?;
        if !status.success() {
            eprintln!("iliada-guest-init: some drivers did not load");
        }

        mount_fs(SPEC_TAG, SPEC_DIR, "virtiofs", MountFlags::RDONLY | nosuid)?;
        let spec: GuestSpec =
            serde_json::from_slice(&std::fs::read(Path::new(SPEC_DIR).join("spec.json")).context("reading spec.json")?)
                .context("parsing spec.json")?;
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

        let status = Command::new(&spec.program)
            .args(&spec.args)
            .env_clear()
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .status()
            .with_context(|| format!("starting {}", spec.program))?;
        Ok(status.code().unwrap_or(1))
    }

    pub fn power_off() -> ! {
        rustix::fs::sync();
        let _ = rustix::system::reboot(rustix::system::RebootCommand::PowerOff);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
}

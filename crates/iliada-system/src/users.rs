//! Who may administer the system: root and members of `wheel`, read from
//! `/etc/passwd` and `/etc/group` (no NSS; Iliada's users are local).

pub const ADMIN_GROUP: &str = "wheel";

fn fields(line: &str) -> Vec<&str> {
    line.split(':').collect()
}

/// `(name, primary gid)` of `uid`.
pub fn user(passwd: &str, uid: u32) -> Option<(String, u32)> {
    passwd
        .lines()
        .map(fields)
        .find(|f| f.len() >= 4 && f[2].parse() == Ok(uid))
        .and_then(|f| Some((f[0].to_owned(), f[3].parse().ok()?)))
}

/// The gid of `group`.
pub fn gid(group_file: &str, group: &str) -> Option<u32> {
    group_file.lines().map(fields).find(|f| f.len() >= 3 && f[0] == group).and_then(|f| f[2].parse().ok())
}

pub fn in_group(passwd: &str, group_file: &str, uid: u32, group: &str) -> bool {
    let Some((name, primary)) = user(passwd, uid) else { return false };
    group_file.lines().map(fields).any(|f| {
        f.len() >= 4 && f[0] == group && (f[2].parse() == Ok(primary) || f[3].split(',').any(|m| m.trim() == name))
    })
}

pub fn is_admin(uid: u32) -> bool {
    if uid == 0 {
        return true;
    }
    let read = |p| std::fs::read_to_string(p).unwrap_or_default();
    in_group(&read("/etc/passwd"), &read("/etc/group"), uid, ADMIN_GROUP)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0::/root:/bin/sh\nalex:x:1000:100::/home/alex:/bin/fish\nsam:x:1001:100::/home/sam:/bin/fish\nop:x:1002:1::/home/op:/bin/sh\n";
    const GROUP: &str = "root:x:0:\nwheel:x:1:alex,bob\nusers:x:100:\n";

    #[test]
    fn wheel_members_and_primary_group() {
        assert!(in_group(PASSWD, GROUP, 1000, "wheel"));
        assert!(!in_group(PASSWD, GROUP, 1001, "wheel"));
        assert!(in_group(PASSWD, GROUP, 1002, "wheel"));
        assert!(!in_group(PASSWD, GROUP, 4242, "wheel"));
        assert_eq!(gid(GROUP, "users"), Some(100));
    }
}

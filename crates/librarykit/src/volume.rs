//! Mount table lookup from `/proc/self/mounts`: the kernel's view, read
//! without touching any mount, so it is safe while an NFS server hangs.

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub mount_point: String,
    /// "ext4", "btrfs", "nfs4", "cifs", "fuse.sshfs", …
    pub file_system: String,
    /// Device or server:/export.
    pub source: String,
}

impl Mount {
    pub fn is_network(&self) -> bool {
        let fs = self.file_system.to_lowercase();
        matches!(
            fs.as_str(),
            "nfs" | "nfs4" | "cifs" | "smb3" | "smbfs" | "afpfs" | "davfs" | "fuse.davfs" | "ftp" | "fuse.sshfs" | "9p"
        ) || fs.starts_with("fuse.smb")
    }
}

/// `/proc/mounts` escapes space, tab, newline and backslash as \ooo.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 4 <= b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_mounts(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split(' ');
            let source = f.next()?;
            let mount_point = f.next()?;
            let file_system = f.next()?;
            Some(Mount {
                mount_point: unescape(mount_point),
                file_system: unescape(file_system),
                source: unescape(source),
            })
        })
        .collect()
}

pub fn mounts() -> Vec<Mount> {
    std::fs::read_to_string("/proc/self/mounts").map(|t| parse_mounts(&t)).unwrap_or_default()
}

/// The mount that holds `path` (longest matching mount point). An automount
/// point is listed as "autofs" and again with the real file system; the
/// real one wins.
pub fn mount_for(path: &Path, list: &[Mount]) -> Option<Mount> {
    let std = crate::standardized(path);
    let std = std.to_string_lossy();
    let matches: Vec<&Mount> = list
        .iter()
        .filter(|m| {
            let prefix =
                if m.mount_point.ends_with('/') { m.mount_point.clone() } else { format!("{}/", m.mount_point) };
            std == m.mount_point || std.starts_with(&prefix)
        })
        .collect();
    let longest = matches.iter().map(|m| m.mount_point.len()).max()?;
    let best: Vec<&&Mount> = matches.iter().filter(|m| m.mount_point.len() == longest).collect();
    best.iter().rev().find(|m| m.file_system != "autofs").or(best.last()).map(|m| (**m).clone())
}

pub fn is_network_volume(path: &Path) -> bool {
    mount_for(path, &mounts()).is_some_and(|m| m.is_network())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "/dev/vda2 / ext4 rw,relatime 0 0
proc /proc proc rw 0 0
systemd-1 /mnt/nas autofs rw 0 0
nas:/volume1/music /mnt/nas nfs4 ro 0 0
//srv/share /mnt/My\\040Share cifs rw 0 0
";

    #[test]
    fn longest_mount_and_autofs() {
        let list = parse_mounts(SAMPLE);
        assert_eq!(mount_for(Path::new("/"), &list).unwrap().mount_point, "/");
        assert_eq!(mount_for(Path::new("/home/x"), &list).unwrap().file_system, "ext4");
        let nas = mount_for(Path::new("/mnt/nas/isos/a.iso"), &list).unwrap();
        assert_eq!(nas.file_system, "nfs4");
        assert!(nas.is_network());
        let share = mount_for(Path::new("/mnt/My Share/x"), &list).unwrap();
        assert_eq!(share.file_system, "cifs");
        assert!(mount_for(Path::new("/mnt/nasty"), &list).unwrap().mount_point == "/");
    }

    #[test]
    fn root_is_listed_on_this_machine() {
        assert!(mounts().iter().any(|m| m.mount_point == "/"));
        assert!(!is_network_volume(&std::env::temp_dir()));
    }
}

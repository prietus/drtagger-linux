//! Moving originals to the trash. Inside the Flatpak the freedesktop trash
//! of the host is reached through the Trash portal (the `trash` crate would
//! fill a trash inside the sandbox's own data folder); outside, directly.

use anyhow::{Result, bail};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub fn sandboxed() -> bool {
    Path::new("/.flatpak-info").exists()
}

pub fn delete(path: &Path) -> Result<()> {
    if !sandboxed() {
        return Ok(trash::delete(path)?);
    }
    // The portal takes a descriptor; O_PATH opens files and folders alike.
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_PATH | libc::O_CLOEXEC).open(path)?;
    let result = crate::runtime().block_on(ashpd::desktop::trash::trash_file(&file));
    if let Err(e) = result {
        bail!("{}: {e}", path.display());
    }
    Ok(())
}

pub fn delete_all(paths: &[impl AsRef<Path>]) -> Result<()> {
    for p in paths {
        delete(p.as_ref())?;
    }
    Ok(())
}

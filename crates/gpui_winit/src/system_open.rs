use std::path::Path;

use anyhow::{Context, Result, ensure};

pub(crate) fn open_url(url: &str) -> Result<()> {
    ensure!(!url.is_empty(), "URL is empty");
    open::that_detached(url).with_context(|| format!("failed to open URL {url}"))
}

pub(crate) fn open_path(path: &Path) -> Result<()> {
    ensure!(!path.as_os_str().is_empty(), "path is empty");
    ensure!(
        path.try_exists()?,
        "path does not exist: {}",
        path.display()
    );
    open::that_detached(path).with_context(|| format!("failed to open {}", path.display()))
}

#[cfg(target_os = "windows")]
pub(crate) fn reveal_path(path: &Path) -> Result<()> {
    use std::{os::windows::process::CommandExt, process::Command};

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    ensure!(!path.as_os_str().is_empty(), "path is empty");
    ensure!(
        path.try_exists()?,
        "path does not exist: {}",
        path.display()
    );
    let path = std::path::absolute(path)
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    if path.is_dir() {
        return open::that_detached(&path)
            .with_context(|| format!("failed to reveal {}", path.display()));
    }
    Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .with_context(|| format!("failed to reveal {} in Explorer", path.display()))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn reveal_path(path: &Path) -> Result<()> {
    ensure!(!path.as_os_str().is_empty(), "path is empty");
    ensure!(
        path.try_exists()?,
        "path does not exist: {}",
        path.display()
    );
    let target = if path.is_dir() {
        path
    } else {
        path.parent().context("path has no parent directory")?
    };
    open::that_detached(target).with_context(|| format!("failed to reveal {}", path.display()))
}

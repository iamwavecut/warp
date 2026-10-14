use command::r#async::Command;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(super) fn valid_repository_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

struct CacheLock(PathBuf);
impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn run_git(args: &[&std::ffi::OsStr]) -> anyhow::Result<()> {
    let output = Command::new("git")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        // Git output may contain credentials from the user's configuration.
        anyhow::bail!(
            "Git checkout operation failed with status {}",
            output.status
        );
    }
    Ok(())
}

/// Refreshes a local full mirror, then creates a checkout with no object-store dependency.
pub(super) async fn clone_with_mirror(url: &str, target: &Path, root: &Path) -> anyhow::Result<()> {
    use std::ffi::OsStr;
    if target.symlink_metadata().is_ok() {
        anyhow::bail!("Checkout target already exists");
    }
    std::fs::create_dir_all(root)?;
    if root.symlink_metadata()?.file_type().is_symlink() {
        anyhow::bail!("Mirror root is a symlink");
    }
    let key = format!("{:x}", Sha256::digest(url.as_bytes()));
    let mirror = root.join(format!("{key}.git"));
    let lock_path = root.join(format!("{key}.lock"));
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)?;
    let _guard = CacheLock(lock_path);
    drop(lock);
    match mirror.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            anyhow::bail!("Invalid mirror cache path")
        }
        Ok(_) => {
            run_git(&[
                OsStr::new("-C"),
                mirror.as_os_str(),
                OsStr::new("remote"),
                OsStr::new("update"),
                OsStr::new("--prune"),
            ])
            .await?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let temporary = tempfile::tempdir_in(root)?;
            let candidate = temporary.path().join("mirror.git");
            run_git(&[
                OsStr::new("clone"),
                OsStr::new("--mirror"),
                OsStr::new("--"),
                OsStr::new(url),
                candidate.as_os_str(),
            ])
            .await?;
            std::fs::rename(candidate, &mirror)?;
        }
        Err(error) => return Err(error.into()),
    }
    run_git(&[
        OsStr::new("clone"),
        OsStr::new("--no-hardlinks"),
        OsStr::new("--"),
        mirror.as_os_str(),
        target.as_os_str(),
    ])
    .await?;
    run_git(&[
        OsStr::new("-C"),
        target.as_os_str(),
        OsStr::new("remote"),
        OsStr::new("set-url"),
        OsStr::new("origin"),
        OsStr::new(url),
    ])
    .await
}

#[cfg(test)]
#[path = "local_checkout_tests.rs"]
mod tests;

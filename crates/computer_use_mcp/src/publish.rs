//! Saving finished recordings locally.

use std::path::{Path, PathBuf};

pub(crate) async fn move_into(video: &Path, directory: &Path) -> Result<PathBuf, String> {
    let saving_error = |error: std::io::Error| format!("Failed to save the recording: {error}.");
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(saving_error)?;
    let file_name = video
        .file_name()
        .ok_or_else(|| "The recording has no file name.".to_owned())?;
    let target = directory.join(file_name);
    // Renaming fails across filesystems, so fall back to copying.
    if tokio::fs::rename(video, &target).await.is_err() {
        tokio::fs::copy(video, &target)
            .await
            .map_err(saving_error)?;
        let _ = tokio::fs::remove_file(video).await;
    }
    Ok(target)
}

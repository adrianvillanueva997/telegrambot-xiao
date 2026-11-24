use std::{io, path::Path, time::Duration};
use tokio::process::Command;
use tracing::instrument;

use crate::online_downloads::video_downloader::MAX_VIDEO_SIZE;

/// Converts a webm to mp4, failing if `FFmpeg` fails or times out.
///
/// # Errors
/// Returns an error when `FFmpeg` cannot run, fails, or exceeds the time limit.
#[instrument]
pub async fn convert_webm_to_mp4(
    webm_filename: impl AsRef<Path> + std::fmt::Debug,
    mp4_filename: impl AsRef<Path> + std::fmt::Debug,
) -> io::Result<()> {
    let status = tokio::time::timeout(
        Duration::from_secs(120),
        Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-y",
                "-protocol_whitelist",
                "file,pipe",
                "-f",
                "matroska",
                "-i",
            ])
            .arg(webm_filename.as_ref())
            .args(["-fs", "20000001"])
            .arg(mp4_filename.as_ref())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await??;
    if !status.success() {
        return Err(io::Error::other(format!("FFmpeg failed: {status}")));
    }
    if tokio::fs::metadata(mp4_filename.as_ref()).await?.len() > MAX_VIDEO_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "converted video exceeds size limit",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn conversion_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            super::convert_webm_to_mp4(dir.path().join("missing.webm"), dir.path().join("out.mp4"))
                .await
                .is_err()
        );
    }
}

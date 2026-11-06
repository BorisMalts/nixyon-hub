use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

const GZIP_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub fn gzip_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".gz");
    PathBuf::from(name)
}

// Сжатая копия появляется под своим именем только целиком: хаб отдаёт её, лишь когда
// она есть, а до того отдаёт сам APK.
pub async fn gzip_beside(file: &Path) -> std::io::Result<PathBuf> {
    let target = gzip_path(file);
    let mut part = target.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let out = std::fs::File::create(&part)?;
    let status = Command::new("gzip")
        .args(["-6", "-n", "-c"])
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    let ok = matches!(
        tokio::time::timeout(GZIP_TIMEOUT, status).await,
        Ok(Ok(s)) if s.success()
    );
    if !ok {
        let _ = std::fs::remove_file(&part);
        return Err(std::io::Error::other("gzip не справился"));
    }
    std::fs::rename(&part, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[tokio::test]
    async fn compressed_copy_unpacks_to_the_original() {
        let dir = std::env::temp_dir().join(format!("nixyon-gzip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let apk = dir.join("a.apk");
        let body = "nixyon ".repeat(10_000);
        std::fs::write(&apk, &body).unwrap();
        let gz = gzip_beside(&apk).await.unwrap();
        assert_eq!(gz, dir.join("a.apk.gz"));
        assert!(std::fs::metadata(&gz).unwrap().len() < body.len() as u64 / 10);
        let mut unpacked = String::new();
        std::process::Command::new("gzip")
            .args(["-d", "-c"])
            .arg(&gz)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
            .stdout
            .unwrap()
            .read_to_string(&mut unpacked)
            .unwrap();
        assert_eq!(unpacked, body);
        assert!(!dir.join("a.apk.gz.part").exists());
    }
}

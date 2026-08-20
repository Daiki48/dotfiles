//! 外部system binaryを起動する前の共通信頼検証。

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[cfg(unix)]
fn trusted_system_uid(uid: u32) -> bool {
    if uid == 0 {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        // user namespaceではhost root所有fileがkernel overflow UIDに見える。
        fs::read_to_string("/proc/sys/kernel/overflowuid")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            == Some(uid)
    }
    #[cfg(not(target_os = "linux"))]
    false
}

#[cfg(unix)]
fn trusted_directory(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_dir()
        && !metadata.file_type().is_symlink()
        && trusted_system_uid(metadata.uid())
        && metadata.mode() & 0o022 == 0
}

#[cfg(unix)]
fn trusted_executable(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_file()
        && !metadata.file_type().is_symlink()
        && trusted_system_uid(metadata.uid())
        && metadata.mode() & 0o022 == 0
        && metadata.mode() & 0o111 != 0
}

pub(crate) fn trusted_system_binary(path: &str, name: &str) -> Result<String, String> {
    let path = Path::new(path);
    if !path.is_absolute() || path.parent() != Some(Path::new("/usr/bin")) {
        return Err(format!("system {name}が固定pathではありません"));
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|_| format!("system {name}を確認できません"))?;
    #[cfg(unix)]
    if !trusted_executable(&metadata) {
        return Err(format!("system {name}が安全な実行fileではありません"));
    }
    #[cfg(not(unix))]
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!("system {name}が安全な実行fileではありません"));
    }

    #[cfg(unix)]
    {
        let root = fs::symlink_metadata("/")
            .map_err(|_| format!("system {name}のroot directoryを確認できません"))?;
        if !trusted_directory(&root) {
            return Err(format!("system {name}のroot directoryが安全ではありません"));
        }
        let mut component = PathBuf::from("/");
        for part in path.parent().into_iter().flat_map(Path::components).skip(1) {
            component.push(part.as_os_str());
            let ancestor = fs::symlink_metadata(&component)
                .map_err(|_| format!("system {name}の祖先directoryを確認できません"))?;
            if !trusted_directory(&ancestor) {
                return Err(format!("system {name}の祖先directoryが安全ではありません"));
            }
        }
    }
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn fixed_system_binaries_and_all_ancestors_are_trusted() {
        for (path, name) in [
            ("/usr/bin/git", "Git"),
            ("/usr/bin/gh", "GitHub CLI"),
            ("/usr/bin/ssh", "SSH"),
        ] {
            assert!(trusted_system_binary(path, name).is_ok(), "{path}");
        }
        #[cfg(target_os = "linux")]
        assert!(trusted_system_binary("/usr/bin/unshare", "Unshare").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn user_owned_and_symlinked_paths_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("codex-system-binary-{suffix}"));
        let executable = root.join("git");
        let link = root.join("git-link");
        fs::create_dir(&root).expect("create trust fixture");
        fs::write(&executable, b"fixture").expect("write fake executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o777))
            .expect("set fake executable mode");
        symlink(&executable, &link).expect("create executable symlink");

        assert!(!trusted_executable(
            &fs::symlink_metadata(&executable).expect("fake executable metadata")
        ));
        if !trusted_system_uid(unsafe { libc::getuid() }) {
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
                .expect("set user-owned executable mode");
            assert!(!trusted_executable(
                &fs::symlink_metadata(&executable).expect("user executable metadata")
            ));
        }
        assert!(!trusted_executable(
            &fs::symlink_metadata(&link).expect("executable symlink metadata")
        ));

        fs::remove_file(link).expect("remove executable symlink");
        fs::remove_file(executable).expect("remove fake executable");
        fs::remove_dir(root).expect("remove trust fixture");
    }
}

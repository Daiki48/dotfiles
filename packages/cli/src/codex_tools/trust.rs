//! 外部system binaryを起動する前の共通信頼検証。

use std::fs;
use std::path::{Path, PathBuf};

/// 書き込みに使う認証 snapshot で本人を照合する。認証応答は表示しない。
pub(crate) fn github_actor(config: &Path, author: Option<&str>) -> Result<(), String> {
    let gh = trusted_system_binary("/usr/bin/gh", "GitHub CLI")?;
    let api = |endpoint: &str| -> Result<serde_json::Value, String> {
        let mut command = std::process::Command::new(&gh);
        super::process::clear_environment(&mut command);
        command
            .current_dir(config)
            .env("HOME", config)
            .env("XDG_STATE_HOME", config)
            .env("GH_CONFIG_DIR", config)
            .env("GH_HOST", "github.com")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("PATH", "/usr/bin:/bin")
            .args(["api", endpoint]);
        let result = super::process::run_with_limit(
            &mut command,
            std::time::Duration::from_secs(5),
            256 * 1024,
        )
        .map_err(|_| "GitHub の本人確認に失敗しました")?;
        if !result.status.success() {
            return Err("GitHub の本人確認に失敗しました".into());
        }
        serde_json::from_slice(&result.stdout).map_err(|_| "GitHub の本人確認応答が不正です".into())
    };
    verify_identity(author, api)
}

fn verify_identity(
    author: Option<&str>,
    api: impl Fn(&str) -> Result<serde_json::Value, String>,
) -> Result<(), String> {
    let user = api("user")?;
    if !user["login"]
        .as_str()
        .is_some_and(|login| login.eq_ignore_ascii_case("Daiki48"))
    {
        return Err("GitHub の認証アカウントが本人と一致しません".into());
    }
    if let Some(email) = author {
        if user["id"].as_u64().is_some_and(|id| {
            email.eq_ignore_ascii_case(&format!("{id}+Daiki48@users.noreply.github.com"))
        }) {
            return Ok(());
        }
        for endpoint in ["user/public_emails", "user/emails"] {
            if api(endpoint)
                .ok()
                .and_then(|v| v.as_array().cloned())
                .is_some_and(|rows| {
                    rows.iter().any(|row| {
                        row["verified"] == true
                            && row["email"]
                                .as_str()
                                .is_some_and(|e| email.eq_ignore_ascii_case(e))
                    })
                })
            {
                return Ok(());
            }
        }
        return Err("Git 著者と本人アカウントの紐付けを確認できません".into());
    }
    Ok(())
}

#[test]
fn writer_and_commit_author_are_checked_separately() {
    let api = |endpoint: &str| {
        Ok(if endpoint == "user" {
            serde_json::json!({"login":"Daiki48","id":123})
        } else {
            serde_json::json!([{"email":"author@example.invalid","verified":true}])
        })
    };
    assert!(verify_identity(None, |_| Ok(serde_json::json!({"login":"other"}))).is_err());
    assert!(verify_identity(None, api).is_ok());
    assert!(verify_identity(Some("author@example.invalid"), api).is_ok());
    assert!(verify_identity(Some("other@example.invalid"), api).is_err());
}

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
            ("/usr/bin/true", "True"),
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

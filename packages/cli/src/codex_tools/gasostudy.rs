//! 今回承認された固定control-planeだけを起動する。汎用shell入口にはしない。

use std::ffi::OsString;

pub(crate) const NAME: &str = "codex-gasostudy-control-plane";
// nonceを含む承認設定全体を固定する。内部識別情報は公開sourceへ入れない。
const PROFILE_SHA256: &str = "ba38d16ebe77aa92c58eed1986922424427dba0a514cfdb8af546e2d35b5f535";

#[derive(serde::Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    nonce: String,
    source: String,
    root: String,
    common: String,
    manifest: String,
    executor: String,
    executor_sha256: String,
    output: String,
    task_id: String,
    repository: String,
    github_name: String,
    branch: String,
    origin: String,
}

impl Profile {
    fn load() -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        {
            linux::load_profile()
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("Linux専用入口です".into())
        }
    }
}

pub(crate) fn operation(args: &[String]) -> Result<&str, String> {
    operation_for(args, &Profile::load()?)
}

fn operation_for<'a>(args: &'a [String], p: &Profile) -> Result<&'a str, String> {
    if args.len() != 5
        || args[1] != "--repository"
        || args[2] != p.root
        || args[3] != "--source"
        || args[4] != p.source
        || !["check", "control-plane-build", "control-plane-validate"].contains(&args[0].as_str())
    {
        return Err("固定operation、--repository <固定worktree>、--source <review済み40桁SHA> だけを指定してください".into());
    }
    Ok(&args[0])
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::codex_tools::{guard, process, trust};
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    fn decode_profile(bytes: &[u8], expected: &str) -> Result<Profile, String> {
        if format!("{:x}", Sha256::digest(bytes)) != expected {
            return Err("承認設定のdigestがinstalled版と一致しません".into());
        }
        let p: Profile =
            serde_json::from_slice(bytes).map_err(|_| "承認設定のschemaが不正です".to_string())?;
        let hex = |s: &str, n| {
            s.len() == n
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        };
        let absolute = |s: &str| {
            Path::new(s).is_absolute()
                && Path::new(s).components().all(|c| {
                    matches!(
                        c,
                        std::path::Component::RootDir | std::path::Component::Normal(_)
                    )
                })
        };
        if p.version != 1
            || !hex(&p.nonce, 64)
            || !hex(&p.source, 40)
            || !hex(&p.executor_sha256, 64)
            || [&p.root, &p.common, &p.manifest, &p.output, &p.repository]
                .iter()
                .any(|s| !absolute(s))
            || p.executor.is_empty()
            || Path::new(&p.executor).is_absolute()
            || !Path::new(&p.executor)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
            || Path::new(&p.common) != Path::new(&p.repository).join(".git")
            || Path::new(&p.root).file_name().and_then(|s| s.to_str()) != Some(p.task_id.as_str())
            || Path::new(&p.output).parent() != Some(Path::new("/tmp"))
        {
            return Err("承認設定の固定identity形式が不正です".into());
        }
        Ok(p)
    }

    pub(super) fn load_profile() -> Result<Profile, String> {
        identity(
            unsafe { libc::getuid() },
            unsafe { libc::geteuid() },
            unsafe { libc::getgid() },
            unsafe { libc::getegid() },
        )?;
        // HOME/CODEX_HOMEやargvから承認fileを選ばせない。
        let home = crate::codex::account_home_dir().map_err(|_| "account homeを確認できません")?;
        let bytes = read_file(&home.join(".codex/control-plane-approval.json"), true)?;
        decode_profile(&bytes, PROFILE_SHA256)
    }

    pub(super) fn preflight(operation: &str) -> Result<Vec<u8>, String> {
        preflight_for(&Profile::load()?, operation)
    }

    fn identity(uid: u32, euid: u32, gid: u32, egid: u32) -> Result<(), String> {
        if (uid, euid, gid, egid) != (1000, 1000, 1000, 1000) {
            return Err("承認済みの同一nonroot UID/GIDが必要です".into());
        }
        Ok(())
    }

    fn directories(path: &Path) -> Result<(), String> {
        let overflow = fs::read_to_string("/proc/sys/kernel/overflowuid")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());
        let mut component = PathBuf::new();
        for part in path.components() {
            component.push(part);
            let m = fs::symlink_metadata(&component).map_err(|e| e.to_string())?;
            if !m.is_dir()
                || m.file_type().is_symlink()
                || !(m.uid() == 0 || m.uid() == 1000 || Some(m.uid()) == overflow)
                || m.mode() & 0o022 != 0
            {
                return Err("directoryのidentity/modeを確認できません".into());
            }
        }
        Ok(())
    }

    fn read_file(path: &Path, private: bool) -> Result<Vec<u8>, String> {
        directories(path.parent().ok_or("fileの親がありません")?)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
            .map_err(|e| e.to_string())?;
        let m = file.metadata().map_err(|e| e.to_string())?;
        if !m.is_file()
            || m.uid() != 1000
            || m.nlink() != 1
            || m.mode() & (if private { 0o077 } else { 0o022 }) != 0
            || m.len() > 1024 * 1024
        {
            return Err("fileのowner/mode/size/linkを確認できません".into());
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 1024 * 1024 {
            return Err("fileのsize上限超過".into());
        }
        Ok(bytes)
    }

    fn git(p: &Profile, args: &[&str]) -> Result<Vec<u8>, String> {
        let mut command = Command::new(trust::trusted_system_binary("/usr/bin/git", "Git")?);
        process::clear_environment(&mut command);
        command
            .current_dir(p.root.as_str())
            .envs([
                ("HOME", "/dev/null"),
                ("PATH", "/usr/bin:/bin"),
                ("LANG", "C"),
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_CONFIG_GLOBAL", "/dev/null"),
                ("GIT_CONFIG_SYSTEM", "/dev/null"),
                ("GIT_TERMINAL_PROMPT", "0"),
                ("GIT_OPTIONAL_LOCKS", "0"),
                ("GIT_NO_LAZY_FETCH", "1"),
            ])
            .args([
                "--no-replace-objects",
                "--no-pager",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "credential.helper=",
                "-c",
                "protocol.allow=never",
            ])
            .args(args);
        let output = process::run_with_limit(&mut command, Duration::from_secs(10), 1024 * 1024)
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err("固定Git検査が失敗しました".into());
        }
        Ok(output.stdout)
    }

    fn git_text(p: &Profile, args: &[&str]) -> Result<String, String> {
        String::from_utf8(git(p, args)?)
            .map(|v| v.trim().to_owned())
            .map_err(|e| e.to_string())
    }

    #[derive(Deserialize)]
    struct Binding {
        version: u32,
        status: String,
        task_id: String,
        repository: String,
        common_git_dir: String,
        github_name: String,
        worktree: String,
        branch: String,
    }

    fn binding(p: &Profile, bytes: &[u8]) -> Result<(), String> {
        let b: Binding = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if b.version != 1
            || b.status != "ready"
            || b.task_id != p.task_id
            || b.repository != p.repository
            || b.common_git_dir != p.common.as_str()
            || b.github_name != p.github_name
            || b.worktree != p.root.as_str()
            || b.branch != p.branch
        {
            return Err("managed taskの固定identityが一致しません".into());
        }
        Ok(())
    }

    fn executor(p: &Profile, bytes: &[u8]) -> Result<(), String> {
        if format!("{:x}", Sha256::digest(bytes)) != p.executor_sha256 {
            return Err("review済みexecutor blobと一致しません".into());
        }
        Ok(())
    }

    fn output(operation: &str, path: &Path, uid: u32) -> Result<(), String> {
        // /tmp自身はstickyなsystem directory。下の固定出力だけを許す。
        let tmp = fs::symlink_metadata(path.parent().ok_or("出力先の親がありません")?)
            .map_err(|e| e.to_string())?;
        if !tmp.is_dir() || tmp.mode() & 0o1777 != 0o1777 {
            return Err("tmpの属性が不正です".into());
        }
        match fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() && m.uid() == uid && m.mode() & 0o7777 == 0o700 => (),
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && operation != "control-plane-validate" =>
            {
                return Ok(());
            }
            _ => return Err("出力directoryのidentityが不正です".into()),
        }
        let artifact = path.join("control-plane.tar.gz");
        match fs::symlink_metadata(&artifact) {
            Ok(m)
                if operation == "control-plane-validate"
                    && m.is_file()
                    && m.uid() == uid
                    && m.mode() & 0o7777 == 0o600
                    && m.nlink() == 1
                    && m.len() > 0
                    && m.len() <= 1048576 =>
            {
                Ok(())
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && operation != "control-plane-validate" =>
            {
                Ok(())
            }
            _ => Err(
                "出力artifactが不正または既存です。上書きせずGasoStudy側で保存・照合してください"
                    .into(),
            ),
        }
    }

    fn preflight_for(p: &Profile, operation: &str) -> Result<Vec<u8>, String> {
        identity(
            unsafe { libc::getuid() },
            unsafe { libc::geteuid() },
            unsafe { libc::getgid() },
            unsafe { libc::getegid() },
        )?;
        directories(Path::new(p.root.as_str()))?;
        directories(Path::new(p.common.as_str()))?;
        binding(p, &read_file(Path::new(p.manifest.as_str()), true)?)?;
        // 実行可能なlocal configを、Gitのsource検査より先に排除する。
        read_file(&Path::new(p.common.as_str()).join("config"), false)?;
        let keys = git_text(
            p,
            &[
                "config",
                "--file",
                &format!("{}/config", p.common),
                "--no-includes",
                "--name-only",
                "--list",
            ],
        )?;
        if keys.lines().any(|key| {
            guard::dangerous_local_git_key(key)
                || key.eq_ignore_ascii_case("extensions.worktreeconfig")
                || key.starts_with("remote.") && key.ends_with(".promisor")
        }) {
            return Err("実行・認証経路を持つGit configを拒否しました".into());
        }
        if git_text(p, &["rev-parse", "--show-toplevel"])? != p.root.as_str()
            || git_text(
                p,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )? != p.common.as_str()
            || git_text(p, &["rev-parse", "HEAD"])? != p.source.as_str()
            || git_text(p, &["rev-parse", "--abbrev-ref", "HEAD"])? != p.branch
            || git_text(p, &["config", "--local", "--get", "remote.origin.url"])? != p.origin
            || !git_text(p, &["status", "--porcelain=v1", "--untracked-files=all"])?.is_empty()
        {
            return Err("repository/common Git/source/branch/cleanの固定条件が一致しません".into());
        }
        let registered = git(p, &["worktree", "list", "--porcelain", "-z"])?;
        if !registered
            .split(|b| *b == 0)
            .any(|row| row == format!("worktree {}", p.root).as_bytes())
        {
            return Err("Git worktree登録が一致しません".into());
        }
        let blob = git(p, &["show", &format!("{}:{}", p.source, p.executor)])?;
        executor(p, &blob)?;
        executor(
            p,
            &read_file(&Path::new(p.root.as_str()).join(p.executor.as_str()), false)?,
        )?;
        trust::trusted_system_binary("/usr/bin/bash", "Bash")?;
        output(operation, Path::new(p.output.as_str()), 1000)?;
        Ok(blob)
    }

    fn command(p: &Profile, operation: &str) -> Command {
        let mut command = Command::new("/usr/bin/bash");
        configure(p, &mut command, operation);
        command
    }

    fn configure(p: &Profile, command: &mut Command, operation: &str) {
        command
            .env_clear()
            .current_dir(p.root.as_str())
            .envs([
                ("GASOSTUDY_SANDBOX_LAUNCH", "1"),
                ("GASOSTUDY_CONTROL_PLANE_SOURCE_SHA", p.source.as_str()),
                ("GASOSTUDY_CONTROL_PLANE_REPOSITORY", p.root.as_str()),
                ("LANG", "C"),
                ("LC_ALL", "C"),
                ("PATH", "/usr/bin:/bin"),
                ("PWD", p.root.as_str()),
            ])
            .args(["--noprofile", "--norc", "-p", "-s", "--", operation]);
    }

    fn sealed_input(bytes: &[u8]) -> Result<File, String> {
        let fd = unsafe {
            libc::memfd_create(
                c"gasostudy-reviewed-executor".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        if unsafe {
            libc::fcntl(
                file.as_raw_fd(),
                libc::F_ADD_SEALS,
                libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(file)
    }

    pub(super) fn run(operation: &str) -> Result<(), String> {
        let p = Profile::load()?;
        let blob = preflight_for(&p, operation)?;
        if operation == "check" {
            println!("承認済みsource/task/executor/出力先の検査成功 (build未実行)");
            return Ok(());
        }
        let input = sealed_input(&blob)?;
        // 検査済みbytesだけをstdinへ固定。executor自身がsandbox/timeout/子孫回収を担当する。
        Err(command(&p, operation)
            .stdin(Stdio::from(input))
            .exec()
            .to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn approval_requires_pinned_bytes_and_strict_schema() {
            let valid = serde_json::to_vec(&fixture_profile()).unwrap();
            let pin = format!("{:x}", Sha256::digest(&valid));
            assert!(decode_profile(&valid, &pin).is_ok());
            assert!(decode_profile(&valid, PROFILE_SHA256).is_err());
            let mut changed = valid.clone();
            changed.push(b' ');
            assert!(decode_profile(&changed, &pin).is_err());
            assert!(
                matches!(decode_profile(b"{}", &pin), Err(e) if e == "承認設定のdigestがinstalled版と一致しません")
            );
        }

        #[test]
        fn approval_rejects_unknown_duplicate_and_malformed_identity() {
            let valid = serde_json::to_value(fixture_profile()).unwrap();
            for (key, bad) in [
                ("extra", serde_json::json!(true)),
                ("version", serde_json::json!(2)),
                ("nonce", serde_json::json!("short")),
                ("source", serde_json::json!("HEAD")),
                ("executor_sha256", serde_json::json!("short")),
                ("root", serde_json::json!("relative/path")),
                ("common", serde_json::json!("/fixture/other/.git")),
                ("manifest", serde_json::json!("/fixture/../other")),
                ("executor", serde_json::json!("../other.sh")),
                ("output", serde_json::json!("/fixture/output")),
                ("task_id", serde_json::json!("other")),
            ] {
                let mut value = valid.clone();
                value[key] = bad;
                let bytes = serde_json::to_vec(&value).unwrap();
                let pin = format!("{:x}", Sha256::digest(&bytes));
                assert!(decode_profile(&bytes, &pin).is_err(), "{key}");
            }
            let bytes = serde_json::to_string(&valid)
                .unwrap()
                .replacen('{', "{\"version\":1,", 1);
            let pin = format!("{:x}", Sha256::digest(bytes.as_bytes()));
            assert!(decode_profile(bytes.as_bytes(), &pin).is_err());
        }
        #[test]
        fn rejects_root_other_uid_and_setid() {
            assert!(identity(1000, 1000, 1000, 1000).is_ok());
            for tuple in [
                (0, 0, 0, 0),
                (1000, 0, 1000, 1000),
                (1001, 1001, 1001, 1001),
                (1000, 1000, 1000, 0),
            ] {
                assert!(identity(tuple.0, tuple.1, tuple.2, tuple.3).is_err());
            }
        }
        #[test]
        fn rejects_replaced_blob_and_binding() {
            let p = fixture_profile();
            assert!(executor(&p, b"echo replaced").is_err());
            let valid = serde_json::json!({"version":1,"status":"ready","task_id":p.task_id,
                "repository":p.repository,"common_git_dir":p.common,"github_name":p.github_name,
                "worktree":p.root,"branch":p.branch});
            assert!(binding(&p, &serde_json::to_vec(&valid).unwrap()).is_ok());
            for key in [
                "status",
                "task_id",
                "repository",
                "common_git_dir",
                "github_name",
                "worktree",
                "branch",
            ] {
                let mut bad = valid.clone();
                bad[key] = "other".into();
                assert!(
                    binding(&p, &serde_json::to_vec(&bad).unwrap()).is_err(),
                    "{key}"
                );
            }
        }
        #[test]
        fn launch_has_exact_credential_free_environment_and_argv() {
            let mut c = Command::new("/usr/bin/bash");
            c.env("GH_TOKEN", "test-only")
                .env("BASH_ENV", "/bad")
                .env("UNKNOWN", "bad");
            configure(&fixture_profile(), &mut c, "control-plane-build");
            assert_eq!(c.get_program(), "/usr/bin/bash");
            assert_eq!(
                c.get_args().collect::<Vec<_>>(),
                [
                    "--noprofile",
                    "--norc",
                    "-p",
                    "-s",
                    "--",
                    "control-plane-build"
                ]
            );
            assert_eq!(c.get_envs().count(), 7);
            for (key, value) in c.get_envs() {
                assert!(
                    [
                        "GASOSTUDY_SANDBOX_LAUNCH",
                        "GASOSTUDY_CONTROL_PLANE_SOURCE_SHA",
                        "GASOSTUDY_CONTROL_PLANE_REPOSITORY",
                        "LANG",
                        "LC_ALL",
                        "PATH",
                        "PWD"
                    ]
                    .contains(&key.to_str().unwrap())
                );
                assert!(value.is_some());
            }
        }
        #[test]
        fn sealed_stdin_cannot_be_replaced_or_written() {
            let mut f = sealed_input(b"reviewed blob").unwrap();
            assert!(f.write_all(b"x").is_err());
            let mut content = String::new();
            f.read_to_string(&mut content).unwrap();
            assert_eq!(content, "reviewed blob");
        }

        #[test]
        fn output_rejects_overwrite_symlink_mode_owner_and_oversize() {
            use std::os::unix::fs::{PermissionsExt, symlink};
            let uid = unsafe { libc::getuid() };
            let root = std::env::temp_dir().join(format!(
                "codex-gasostudy-output-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            assert!(output("control-plane-build", &root, uid).is_ok());
            assert!(output("control-plane-validate", &root, uid).is_err());
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            assert!(output("control-plane-build", &root, uid).is_ok());
            assert!(output("control-plane-build", &root, uid + 1).is_err());
            let artifact = root.join("control-plane.tar.gz");
            fs::write(&artifact, b"test artifact").unwrap();
            fs::set_permissions(&artifact, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(output("control-plane-build", &root, uid).is_err());
            assert!(output("control-plane-validate", &root, uid).is_ok());
            fs::set_permissions(&artifact, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(output("control-plane-validate", &root, uid).is_err());
            fs::set_permissions(&artifact, fs::Permissions::from_mode(0o600)).unwrap();
            File::options()
                .write(true)
                .open(&artifact)
                .unwrap()
                .set_len(1048577)
                .unwrap();
            assert!(output("control-plane-validate", &root, uid).is_err());
            fs::remove_file(&artifact).unwrap();
            symlink("/dev/null", &artifact).unwrap();
            assert!(output("control-plane-validate", &root, uid).is_err());
            assert!(output("control-plane-build", &root, uid).is_err());
            fs::remove_file(artifact).unwrap();
            fs::remove_dir(&root).unwrap();
            symlink("/tmp", &root).unwrap();
            assert!(output("control-plane-build", &root, uid).is_err());
            fs::remove_file(root).unwrap();
        }

        #[test]
        #[ignore = "承認済みGasoStudy hostの固定sourceを読み取り検査する"]
        fn approved_host_preflight_without_build() {
            let p = Profile::load().unwrap();
            if Path::new(&p.output)
                .join("control-plane.tar.gz")
                .try_exists()
                .unwrap()
            {
                // 既存artifactを保護したままvalidateの事前検査だけを行う。
                assert!(preflight_for(&p, "check").is_err());
                preflight_for(&p, "control-plane-validate").unwrap();
            } else {
                preflight_for(&p, "check").unwrap();
            }
        }
    }
}

pub(crate) fn preflight(operation: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::preflight(operation).map(|_| ())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = operation;
        Err("Linux専用入口です".into())
    }
}

pub(crate) fn entrypoint<I: IntoIterator<Item = OsString>>(args: I) -> i32 {
    let result = args
        .into_iter()
        .map(|v| {
            v.into_string()
                .map_err(|_| "UTF-8 argvが必要です".to_string())
        })
        .collect::<Result<Vec<_>, _>>()
        .and_then(|args| {
            let operation = operation(&args)?;
            #[cfg(target_os = "linux")]
            {
                linux::run(operation)
            }
            #[cfg(not(target_os = "linux"))]
            {
                preflight(operation)
            }
        });
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{NAME}: {error}");
            2
        }
    }
}

#[cfg(test)]
pub(crate) const TEST_ROOT: &str = "/fixture/worktrees/task-reviewed";
#[cfg(test)]
pub(crate) const TEST_SOURCE: &str = "1111111111111111111111111111111111111111";

#[cfg(test)]
fn fixture_profile() -> Profile {
    serde_json::from_value(serde_json::json!({
        "version":1, "nonce":"a".repeat(64), "source": TEST_SOURCE, "root": TEST_ROOT,
        "common":"/fixture/repository/.git", "manifest":"/fixture/worktrees/.state/task-reviewed.json",
        "executor":"tools/executor.sh", "executor_sha256":"b".repeat(64), "output":"/tmp/control-plane-fixture",
        "task_id":"task-reviewed", "repository":"/fixture/repository", "github_name":"fixture/repository",
        "branch":"fix/reviewed", "origin":"https://github.com/fixture/repository.git"
    })).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    const ROOT: &str = TEST_ROOT;
    const SOURCE: &str = TEST_SOURCE;
    #[test]
    fn only_fixed_source_and_operations_are_accepted() {
        for id in ["check", "control-plane-build", "control-plane-validate"] {
            assert!(
                operation_for(
                    &[id, "--repository", ROOT, "--source", SOURCE].map(str::to_string),
                    &fixture_profile()
                )
                .is_ok()
            );
        }
        for args in [
            vec!["python-policy", "--repository", ROOT, "--source", SOURCE],
            vec![
                "control-plane-build",
                "--repository",
                ROOT,
                "--source",
                "HEAD",
            ],
            vec![
                "control-plane-build",
                "--repository",
                ROOT,
                "--source",
                "1111111",
            ],
            vec![
                "control-plane-build",
                "--repository",
                "/tmp",
                "--source",
                SOURCE,
            ],
            vec![
                "control-plane-build",
                "--repository",
                ROOT,
                "--source",
                "0000000000000000000000000000000000000000",
            ],
            vec![
                "control-plane-build",
                "--repository",
                ROOT,
                "--source",
                SOURCE,
                "extra",
            ],
        ] {
            assert!(
                operation_for(
                    &args.into_iter().map(str::to_string).collect::<Vec<_>>(),
                    &fixture_profile()
                )
                .is_err()
            );
        }
    }
}

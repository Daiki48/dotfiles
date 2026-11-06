//! 今回承認された固定control-planeだけを起動する。汎用shell入口にはしない。

use std::ffi::OsString;

pub(crate) const NAME: &str = "codex-gasostudy-control-plane";
pub(crate) const SOURCE: &str = "c7c40be69cfcb0115fd2c4a8b8f2641ccb3842cf";
pub(crate) const ROOT: &str =
    "/home/daiki/.codex/worktrees/7-daiki48--9-gasostudy/task-280-gasolens-ci";
const COMMON: &str = "/media/p3plus/dev/gasostudy/.git";
const MANIFEST: &str =
    "/home/daiki/.codex/worktrees/7-daiki48--9-gasostudy/.state/task-280-gasolens-ci.json";
const EXECUTOR: &str = "tools/python_sandbox_executor.sh";
const EXECUTOR_SHA256: &str = "be616f5c39dadc5e8bb5f6eff808e5f49df6a5d3e32fcae752605e0869e42fb0";
const OUTPUT: &str = "/tmp/gasostudy-runner-control-plane";

pub(crate) fn operation(args: &[String]) -> Result<&str, String> {
    if args.len() != 3
        || args[1] != "--source"
        || args[2] != SOURCE
        || !["check", "control-plane-build", "control-plane-validate"].contains(&args[0].as_str())
    {
        return Err("固定operationと --source <review済み40桁SHA> だけを指定してください".into());
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

    fn git(args: &[&str]) -> Result<Vec<u8>, String> {
        let mut command = Command::new(trust::trusted_system_binary("/usr/bin/git", "Git")?);
        process::clear_environment(&mut command);
        command
            .current_dir(ROOT)
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

    fn git_text(args: &[&str]) -> Result<String, String> {
        String::from_utf8(git(args)?)
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

    fn binding(bytes: &[u8]) -> Result<(), String> {
        let b: Binding = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if b.version != 1
            || b.status != "ready"
            || b.task_id != "task-280-gasolens-ci"
            || b.repository != "/media/p3plus/dev/gasostudy"
            || b.common_git_dir != COMMON
            || b.github_name != "Daiki48/gasostudy"
            || b.worktree != ROOT
            || b.branch != "fix/gasolens-ci-alignment"
        {
            return Err("managed taskの固定identityが一致しません".into());
        }
        Ok(())
    }

    fn executor(bytes: &[u8]) -> Result<(), String> {
        if format!("{:x}", Sha256::digest(bytes)) != EXECUTOR_SHA256 {
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

    pub(super) fn preflight(operation: &str) -> Result<Vec<u8>, String> {
        identity(
            unsafe { libc::getuid() },
            unsafe { libc::geteuid() },
            unsafe { libc::getgid() },
            unsafe { libc::getegid() },
        )?;
        directories(Path::new(ROOT))?;
        directories(Path::new(COMMON))?;
        binding(&read_file(Path::new(MANIFEST), true)?)?;
        // 実行可能なlocal configを、Gitのsource検査より先に排除する。
        read_file(&Path::new(COMMON).join("config"), false)?;
        let keys = git_text(&[
            "config",
            "--file",
            &format!("{COMMON}/config"),
            "--no-includes",
            "--name-only",
            "--list",
        ])?;
        if keys.lines().any(|key| {
            guard::dangerous_local_git_key(key)
                || key.eq_ignore_ascii_case("extensions.worktreeconfig")
                || key.starts_with("remote.") && key.ends_with(".promisor")
        }) {
            return Err("実行・認証経路を持つGit configを拒否しました".into());
        }
        if git_text(&["rev-parse", "--show-toplevel"])? != ROOT
            || git_text(&["rev-parse", "--path-format=absolute", "--git-common-dir"])? != COMMON
            || git_text(&["rev-parse", "HEAD"])? != SOURCE
            || git_text(&["rev-parse", "--abbrev-ref", "HEAD"])? != "fix/gasolens-ci-alignment"
            || git_text(&["config", "--local", "--get", "remote.origin.url"])?
                != "https://github.com/Daiki48/gasostudy.git"
            || !git_text(&["status", "--porcelain=v1", "--untracked-files=all"])?.is_empty()
        {
            return Err("repository/common Git/source/branch/cleanの固定条件が一致しません".into());
        }
        let registered = git(&["worktree", "list", "--porcelain", "-z"])?;
        if !registered
            .split(|b| *b == 0)
            .any(|row| row == format!("worktree {ROOT}").as_bytes())
        {
            return Err("Git worktree登録が一致しません".into());
        }
        let blob = git(&["show", &format!("{SOURCE}:{EXECUTOR}")])?;
        executor(&blob)?;
        executor(&read_file(&Path::new(ROOT).join(EXECUTOR), false)?)?;
        trust::trusted_system_binary("/usr/bin/bash", "Bash")?;
        output(operation, Path::new(OUTPUT), 1000)?;
        Ok(blob)
    }

    fn command(operation: &str) -> Command {
        let mut command = Command::new("/usr/bin/bash");
        configure(&mut command, operation);
        command
    }

    fn configure(command: &mut Command, operation: &str) {
        command
            .env_clear()
            .current_dir(ROOT)
            .envs([
                ("GASOSTUDY_SANDBOX_LAUNCH", "1"),
                ("GASOSTUDY_CONTROL_PLANE_SOURCE_SHA", SOURCE),
                ("GASOSTUDY_CONTROL_PLANE_REPOSITORY", ROOT),
                ("LANG", "C"),
                ("LC_ALL", "C"),
                ("PATH", "/usr/bin:/bin"),
                ("PWD", ROOT),
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
        if std::env::current_dir().map_err(|e| e.to_string())? != Path::new(ROOT) {
            return Err("固定managed worktreeから実行してください".into());
        }
        let blob = preflight(operation)?;
        if operation == "check" {
            println!("固定source/task/executor/出力先の検査成功: {SOURCE} (build未実行)");
            return Ok(());
        }
        let input = sealed_input(&blob)?;
        // 検査済みbytesだけをstdinへ固定。executor自身がsandbox/timeout/子孫回収を担当する。
        Err(command(operation)
            .stdin(Stdio::from(input))
            .exec()
            .to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
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
            assert!(executor(b"echo replaced").is_err());
            let valid = serde_json::json!({"version":1,"status":"ready","task_id":"task-280-gasolens-ci",
                "repository":"/media/p3plus/dev/gasostudy","common_git_dir":COMMON,"github_name":"Daiki48/gasostudy",
                "worktree":ROOT,"branch":"fix/gasolens-ci-alignment"});
            assert!(binding(&serde_json::to_vec(&valid).unwrap()).is_ok());
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
                    binding(&serde_json::to_vec(&bad).unwrap()).is_err(),
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
            configure(&mut c, "control-plane-build");
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
            preflight("check").unwrap();
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
mod tests {
    use super::*;
    #[test]
    fn only_fixed_source_and_operations_are_accepted() {
        for id in ["check", "control-plane-build", "control-plane-validate"] {
            assert!(operation(&[id, "--source", SOURCE].map(str::to_string)).is_ok());
        }
        for args in [
            vec!["python-policy", "--source", SOURCE],
            vec!["control-plane-build", "--source", "HEAD"],
            vec!["control-plane-build", "--source", "c7c40be"],
            vec![
                "control-plane-build",
                "--source",
                "0000000000000000000000000000000000000000",
            ],
            vec!["control-plane-build", "--source", SOURCE, "extra"],
        ] {
            assert!(operation(&args.into_iter().map(str::to_string).collect::<Vec<_>>()).is_err());
        }
    }
}

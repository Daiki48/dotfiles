//! タスク所有の使い捨て成果物。ソースや任意の外部pathは登録できない。
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Owner {
    version: u32,
    task_id: String,
    worktree: PathBuf,
    common_git_dir: PathBuf,
    device: u64,
    inode: u64,
}

fn paths(worktree: &Path, task: &str) -> Result<(PathBuf, PathBuf)> {
    if !super::worktree::valid_artifact_task_id(task)
        || worktree.file_name().and_then(|v| v.to_str()) != Some(task)
        || !worktree.is_absolute()
    {
        bail!("成果物のtask/pathが一致しません");
    }
    let parent = worktree.parent().context("taskの管理rootがありません")?;
    if fs::canonicalize(parent)? != parent {
        bail!("成果物の管理rootにsymlinkがあります");
    }
    Ok((
        parent.join(".artifacts").join(task),
        parent.join(".state").join(format!("{task}.artifacts.json")),
    ))
}

fn private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || fs::canonicalize(path)? != path {
        bail!("成果物directoryが安全ではありません: {}", path.display());
    }
    #[cfg(unix)]
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        bail!("成果物directoryの所有者またはmodeが不正です");
    }
    Ok(())
}

fn identity(path: &Path) -> Result<(u64, u64)> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    return Ok((metadata.dev(), metadata.ino()));
    #[cfg(not(unix))]
    {
        let _ = metadata;
        bail!("成果物のinode検証はこのOSで未対応です")
    }
}

fn read_owner(marker: &Path) -> Result<Owner> {
    private_directory(marker.parent().context("state rootがありません")?)?;
    let metadata = fs::symlink_metadata(marker)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16 * 1024 {
        bail!("成果物markerが安全なfileではありません");
    }
    #[cfg(unix)]
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        bail!("成果物markerの所有者、mode、link数が不正です");
    }
    Ok(serde_json::from_slice(&fs::read(marker)?)?)
}

/// markerがない既存directoryは採用しない。削除途中の再実行は許可する。
pub(crate) fn validate(worktree: &Path, task: &str, common: &Path) -> Result<Option<PathBuf>> {
    let (path, marker) = paths(worktree, task)?;
    if !marker.try_exists()? && !marker.is_symlink() {
        if path.try_exists()? || path.is_symlink() {
            bail!("未登録の成果物directoryは保持します: {}", path.display());
        }
        return Ok(None);
    }
    let owner = read_owner(&marker)?;
    if owner.version != 1
        || owner.task_id != task
        || owner.worktree != worktree
        || owner.common_git_dir != common
    {
        bail!("成果物markerとtask manifestが一致しません");
    }
    private_directory(path.parent().context("artifacts rootがありません")?)?;
    if path.try_exists()? || path.is_symlink() {
        private_directory(&path)?;
        if identity(&path)? != (owner.device, owner.inode) {
            bail!("成果物directoryが登録後に置換されています");
        }
        crate::clean_disk::validate_artifact_tree(&path)?;
    }
    Ok(Some(path))
}

pub(crate) fn create(worktree: &Path, task: &str, common: &Path) -> Result<PathBuf> {
    if let Some(path) = validate(worktree, task, common)? {
        if !path.exists() {
            bail!("成果物の削除が中断しています。clean-artifactsで完了してください");
        }
        return Ok(path);
    }
    let (path, marker) = paths(worktree, task)?;
    private_directory(marker.parent().context("state rootがありません")?)?;
    let parent = path.parent().context("artifacts rootがありません")?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    if !parent.try_exists()? {
        builder.create(parent)?;
    }
    private_directory(parent)?;
    builder.create(&path)?;
    let (device, inode) = identity(&path)?;
    let owner = Owner {
        version: 1,
        task_id: task.into(),
        worktree: worktree.into(),
        common_git_dir: common.into(),
        device,
        inode,
    };
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&marker)?;
    file.write_all(&serde_json::to_vec_pretty(&owner)?)?;
    file.sync_all()?;
    Ok(path)
}

/// 呼出元がtask lifecycle lockと終了条件を確認した後だけ実行する。
pub(crate) fn cleanup(worktree: &Path, task: &str, common: &Path) -> Result<()> {
    let Some(path) = validate(worktree, task, common)? else {
        return Ok(());
    };
    if path.exists() {
        crate::clean_disk::remove_artifact_tree(&path)?;
    }
    let (_, marker) = paths(worktree, task)?;
    fs::remove_file(marker)?;
    Ok(())
}

/// 通常cleanupで検証できない旧成果物とmarkerを、削除せず対で保持する。
/// 呼出元はrepository lifecycle lockを保持する。移動は同じfilesystem内だけ。
pub(crate) fn archive(worktree: &Path, task: &str, common: &Path) -> Result<PathBuf> {
    if !cfg!(target_os = "linux") {
        bail!("非破壊の成果物退避はLinuxでのみ対応しています");
    }
    archive_with_idle_check(
        worktree,
        task,
        common,
        &crate::clean_disk::ensure_artifact_idle,
    )
}

fn archive_with_idle_check(
    worktree: &Path,
    task: &str,
    common: &Path,
    idle: &dyn Fn(&Path) -> Result<()>,
) -> Result<PathBuf> {
    let (path, marker) = paths(worktree, task)?;
    let parent = worktree.parent().context("管理rootがありません")?;
    private_directory(parent)?;
    private_directory(path.parent().context("artifacts rootがありません")?)?;
    let archives = parent.join(".artifact-archives");
    let destination = archives.join(task);
    let snapshot = destination.join("owner.json");
    let saved_marker = destination.join("registered-owner.json");
    let data = destination.join("data");
    let exists = |p: &Path| -> Result<bool> {
        match fs::symlink_metadata(p) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    };
    let check_owner = |owner: &Owner| -> Result<()> {
        if owner.version != 1
            || owner.task_id != task
            || owner.worktree != worktree
            || owner.common_git_dir != common
        {
            bail!("退避するmarkerとtask manifestが一致しません");
        }
        Ok(())
    };
    let check_data = |p: &Path, owner: &Owner| -> Result<()> {
        private_directory(p)?;
        let (device, inode) = identity(p)?;
        // device番号の変化だけを扱う。異なるinodeの内容は採用しない。
        if inode != owner.inode || device != identity(parent)?.0 {
            bail!("退避対象のinodeまたはfilesystemが一致しません");
        }
        crate::clean_disk::reject_mounted_tree(p)?;
        idle(p)
    };
    if exists(&archives)? {
        private_directory(&archives)?;
    }
    let owner = if exists(&destination)? {
        private_directory(&destination)?;
        let owner = read_owner(&snapshot)?;
        check_owner(&owner)?;
        if exists(&saved_marker)? {
            if exists(&marker)? || read_owner(&saved_marker)? != owner {
                bail!("退避markerの重複または不一致があります");
            }
        } else if read_owner(&marker)? != owner {
            bail!("中断した退避とmarkerが一致しません");
        }
        owner
    } else {
        let owner = read_owner(&marker)?;
        check_owner(&owner)?;
        check_data(&path, &owner)?;
        if identity(&path)?.0 == owner.device {
            bail!("device番号が変化した旧成果物だけを退避できます");
        }
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        builder.mode(0o700);
        if !exists(&archives)? {
            builder.create(&archives)?;
        }
        private_directory(&archives)?;
        builder.create(&destination)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&snapshot)?;
        // marker原本のbytesを保持し、既存markerを編集しない。
        file.write_all(&fs::read(&marker)?)?;
        file.sync_all()?;
        sync_directory(&destination)?;
        sync_directory(&archives)?;
        sync_directory(parent)?;
        owner
    };
    if exists(&data)? {
        if exists(&path)? {
            bail!("退避元と退避先が両方存在します。保持します");
        }
        if !exists(&saved_marker)? {
            bail!("退避先の元markerがありません");
        }
        check_data(&data, &owner)?;
    } else {
        check_data(&path, &owner)?;
        // markerを先に移し、中断中の通常create/cleanupを未登録directoryとして拒否させる。
        if !exists(&saved_marker)? {
            if read_owner(&marker)? != owner {
                bail!("退避中にmarkerが変化しました");
            }
            rename_without_replace(&marker, &saved_marker)?;
            sync_directory(marker.parent().context("state rootがありません")?)?;
            sync_directory(&destination)?;
        }
        rename_without_replace(&path, &data)?;
        sync_directory(path.parent().context("artifacts rootがありません")?)?;
        sync_directory(&destination)?;
    }
    Ok(destination)
}

fn sync_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}

fn rename_without_replace(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (source, destination);
        bail!("非破壊の成果物退避はLinuxでのみ対応しています")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "task-artifacts-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(path.join(".state")).unwrap();
            fs::set_permissions(path.join(".state"), fs::Permissions::from_mode(0o700)).unwrap();
            fs::create_dir(path.join("task-test")).unwrap();
            fs::write(path.join("task-test/source"), "変更済みsource").unwrap();
            Self(path)
        }
        fn worktree(&self) -> PathBuf {
            self.0.join("task-test")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn drift_fixture() -> (Fixture, PathBuf, PathBuf) {
        let f = Fixture::new();
        fs::set_permissions(&f.0, fs::Permissions::from_mode(0o700)).unwrap();
        let common = f.0.join("git");
        let path = create(&f.worktree(), "task-test", &common).unwrap();
        let marker = paths(&f.worktree(), "task-test").unwrap().1;
        let mut owner = read_owner(&marker).unwrap();
        owner.device += 1;
        fs::write(marker, serde_json::to_vec(&owner).unwrap()).unwrap();
        (f, common, path)
    }

    #[test]
    fn archive_preserves_drifted_data_and_original_marker_without_weakening_cleanup() {
        let (f, common, path) = drift_fixture();
        fs::write(path.join("evidence"), "保持する内容").unwrap();
        let marker = paths(&f.worktree(), "task-test").unwrap().1;
        let original_marker = fs::read(&marker).unwrap();
        let original_inode = identity(&path).unwrap().1;
        assert!(cleanup(&f.worktree(), "task-test", &common).is_err());
        let archived =
            archive_with_idle_check(&f.worktree(), "task-test", &common, &|_| Ok(())).unwrap();
        assert!(!path.exists());
        assert!(!marker.exists());
        assert_eq!(
            fs::read(archived.join("registered-owner.json")).unwrap(),
            original_marker
        );
        assert_eq!(identity(&archived.join("data")).unwrap().1, original_inode);
        assert_eq!(
            fs::read_to_string(archived.join("data/evidence")).unwrap(),
            "保持する内容"
        );
        assert_eq!(
            archive_with_idle_check(&f.worktree(), "task-test", &common, &|_| Ok(())).unwrap(),
            archived
        );
        let new_path = create(&f.worktree(), "task-test", &common).unwrap();
        assert!(archive_with_idle_check(&f.worktree(), "task-test", &common, &|_| Ok(())).is_err());
        cleanup(&f.worktree(), "task-test", &common).unwrap();
        assert!(!new_path.exists());
        assert!(archived.join("data/evidence").exists());
        assert!(f.worktree().join("source").exists());
    }

    #[test]
    fn archive_resumes_marker_first_interruption_and_keeps_normal_helpers_closed() {
        let (f, common, path) = drift_fixture();
        let marker = paths(&f.worktree(), "task-test").unwrap().1;
        let dest = f.0.join(".artifact-archives/task-test");
        fs::create_dir_all(&dest).unwrap();
        for p in [dest.clone(), dest.parent().unwrap().into()] {
            fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::copy(&marker, dest.join("owner.json")).unwrap();
        fs::rename(&marker, dest.join("registered-owner.json")).unwrap();
        assert!(create(&f.worktree(), "task-test", &common).is_err());
        assert!(cleanup(&f.worktree(), "task-test", &common).is_err());
        archive_with_idle_check(&f.worktree(), "task-test", &common, &|_| Ok(())).unwrap();
        assert!(!path.exists());
        assert!(dest.join("data").is_dir());
    }

    #[test]
    fn archive_rejects_use_replacement_and_foreign_marker_without_mutation() {
        let (f, common, path) = drift_fixture();
        assert!(
            archive_with_idle_check(&f.worktree(), "task-test", &common, &|_| bail!("使用中"))
                .is_err()
        );
        assert!(
            archive_with_idle_check(&f.worktree(), "task-test", Path::new("/other"), &|_| Ok(()))
                .is_err()
        );
        let preserved = f.0.join("preserved");
        fs::rename(&path, &preserved).unwrap();
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(archive_with_idle_check(&f.worktree(), "task-test", &common, &|_| Ok(())).is_err());
        assert!(path.is_dir() && preserved.is_dir());
        assert!(!f.0.join(".artifact-archives").exists());
    }

    #[test]
    fn archive_preserves_special_files_and_symlinks_without_following_them() {
        use std::os::unix::ffi::OsStrExt;
        let (f, common, path) = drift_fixture();
        let fifo = std::ffi::CString::new(path.join("fifo").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        symlink(f.worktree().join("source"), path.join("external-link")).unwrap();
        let archived =
            archive_with_idle_check(&f.worktree(), "task-test", &common, &|_| Ok(())).unwrap();
        assert!(archived.join("data/external-link").is_symlink());
        assert_eq!(
            fs::symlink_metadata(archived.join("data/fifo"))
                .unwrap()
                .mode()
                & libc::S_IFMT,
            libc::S_IFIFO
        );
        assert!(f.worktree().join("source").exists());
    }

    #[test]
    fn cleanup_is_idempotent_and_preserves_source_and_other_tasks() {
        let f = Fixture::new();
        let common = f.0.join("git");
        let path = create(&f.worktree(), "task-test", &common).unwrap();
        assert_eq!(create(&f.worktree(), "task-test", &common).unwrap(), path);
        fs::write(path.join("vm.img"), "temporary").unwrap();
        fs::create_dir(f.0.join(".artifacts/task-other")).unwrap();
        cleanup(&f.worktree(), "task-test", &common).unwrap();
        cleanup(&f.worktree(), "task-test", &common).unwrap();
        assert!(!path.exists());
        assert!(f.0.join(".artifacts/task-other").exists());
        assert_eq!(
            fs::read_to_string(f.worktree().join("source")).unwrap(),
            "変更済みsource"
        );
    }

    #[test]
    fn unregistered_wrong_owner_and_symlink_are_not_removed() {
        let f = Fixture::new();
        let common = f.0.join("git");
        let path = create(&f.worktree(), "task-test", &common).unwrap();
        assert!(cleanup(&f.worktree(), "task-test", Path::new("/different")).is_err());
        let preserved = f.0.join("preserved");
        fs::rename(&path, &preserved).unwrap();
        symlink(&preserved, &path).unwrap();
        assert!(cleanup(&f.worktree(), "task-test", &common).is_err());
        fs::remove_file(&path).unwrap();
        fs::rename(&preserved, &path).unwrap();
        fs::remove_file(paths(&f.worktree(), "task-test").unwrap().1).unwrap();
        assert!(cleanup(&f.worktree(), "task-test", &common).is_err());
        assert!(path.exists());
        assert!(create(&f.worktree(), "../task-test", &common).is_err());
    }

    #[test]
    fn open_artifact_is_preserved_until_its_user_closes_it() {
        let f = Fixture::new();
        let common = f.0.join("git");
        let path = create(&f.worktree(), "task-test", &common).unwrap();
        fs::write(path.join("in-use"), "temporary").unwrap();
        let open = fs::File::open(path.join("in-use")).unwrap();
        assert!(cleanup(&f.worktree(), "task-test", &common).is_err());
        assert!(path.join("in-use").exists());
        drop(open);
        cleanup(&f.worktree(), "task-test", &common).unwrap();
    }

    #[test]
    fn replaced_directory_and_public_marker_are_preserved() {
        let f = Fixture::new();
        let common = f.0.join("git");
        let path = create(&f.worktree(), "task-test", &common).unwrap();
        let old = f.0.join("old-artifacts");
        fs::rename(&path, &old).unwrap();
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(cleanup(&f.worktree(), "task-test", &common).is_err());
        assert!(path.exists());
        fs::remove_dir(&path).unwrap();
        fs::rename(&old, &path).unwrap();
        let marker = paths(&f.worktree(), "task-test").unwrap().1;
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(cleanup(&f.worktree(), "task-test", &common).is_err());
        assert!(path.exists());
    }

    #[test]
    fn artifact_symlink_does_not_delete_its_target() {
        let f = Fixture::new();
        let common = f.0.join("git");
        let path = create(&f.worktree(), "task-test", &common).unwrap();
        symlink(f.worktree(), path.join("external")).unwrap();
        cleanup(&f.worktree(), "task-test", &common).unwrap();
        assert!(f.worktree().join("source").exists());
    }
}

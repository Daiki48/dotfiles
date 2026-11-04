//! ホスト内の永続状態。短い状態lockとworkerの生存lockを分ける。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::{
    fd::AsRawFd,
    unix::fs::{MetadataExt, OpenOptionsExt},
};

use super::{Result, Task};

const MAX_STATE: u64 = 1024 * 1024;

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn private_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        let metadata =
            fs::symlink_metadata(parent).map_err(|_| "管理directoryの親を確認できません")?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("管理directoryの親が安全ではありません".into());
        }
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("管理pathが通常directoryではありません".into());
            }
            #[cfg(unix)]
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
                return Err("管理directoryの所有者・権限が安全ではありません".into());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return private_dir(path);
                }
                Err(_) => return Err("管理directoryを作成できません".into()),
            }
        }
        Err(_) => return Err("管理directoryを検査できません".into()),
    }
    Ok(())
}

pub(super) fn open_private(path: &Path, create: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(create).create(create);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = options
        .open(path)
        .map_err(|_| "管理fileを安全に開けません")?;
    let metadata = file.metadata().map_err(|_| "管理fileを検査できません")?;
    if !metadata.is_file() {
        return Err("管理fileが通常fileではありません".into());
    }
    #[cfg(unix)]
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err("管理fileの所有者・権限・link数が安全ではありません".into());
    }
    Ok(file)
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let mut file = open_private(path, false)?;
    if file.metadata().map_err(|_| "状態を検査できません")?.len() > MAX_STATE {
        return Err("状態fileが上限を超えています".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_STATE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "状態を読み取れません")?;
    if bytes.len() as u64 > MAX_STATE {
        return Err("状態fileが上限を超えています".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "状態fileの形式が不正です".into())
}

pub(super) fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "状態を生成できません")?;
    if bytes.len() as u64 > MAX_STATE {
        return Err("状態fileが上限を超えています".into());
    }
    let parent = path.parent().ok_or("状態の親directoryがありません")?;
    private_dir(parent)?;
    if path.try_exists().map_err(|_| "保存先を確認できません")? {
        open_private(path, false)?;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".write-{}-{nonce}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(&temporary)
        .map_err(|_| "状態の一時fileを作成できません")?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "状態を保存できません")?;
    fs::rename(&temporary, path).map_err(|_| "状態をatomic保存できません")?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "状態directoryを保存できません")?;
    Ok(())
}

pub(super) struct Lock {
    file: File,
}

impl Lock {
    pub(super) fn acquire_short(path: &Path) -> Result<Self> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(lock) = Self::try_acquire(path)? {
                return Ok(lock);
            }
            if std::time::Instant::now() >= deadline {
                return Err("短い状態更新lockが使用中です。再試行してください".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[cfg(unix)]
    pub(super) fn fd(&self) -> i32 {
        self.file.as_raw_fd()
    }

    #[cfg(unix)]
    pub(super) unsafe fn from_inherited(fd: i32, path: &Path) -> Result<Self> {
        use std::os::fd::FromRawFd;
        if fd < 3 {
            return Err("worker lock descriptorが不正です".into());
        }
        // 呼出側はこのdescriptorが専用workerへ継承されたことを確認する。
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata().map_err(|_| "worker lockを検査できません")?;
        let expected = open_private(path, false)?
            .metadata()
            .map_err(|_| "worker lockを検査できません")?;
        if metadata.dev() != expected.dev()
            || metadata.ino() != expected.ino()
            || unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0
        {
            return Err("継承したworker lockが一致しません".into());
        }
        Ok(Self { file })
    }
    pub(super) fn try_acquire(path: &Path) -> Result<Option<Self>> {
        let file = open_private(path, true)?;
        #[cfg(unix)]
        {
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    return Ok(None);
                }
                return Err("排他lockを取得できません".into());
            }
        }
        #[cfg(not(unix))]
        return Err("このOSの排他lockは未対応です".into());
        #[allow(unreachable_code)]
        Ok(Some(Self { file }))
    }

    /// workerが異常終了しても、その子が残る間は新writerを許可しない。
    pub(super) fn inherit(&self) -> Result<()> {
        #[cfg(unix)]
        {
            let fd = self.file.as_raw_fd();
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
            {
                return Err("worker生存lockを継承できません".into());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        Err("このOSのworker監督は未対応です".into())
    }

    pub(super) fn close_on_exec(&self) -> Result<()> {
        #[cfg(unix)]
        if unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err("coordinator lockの継承設定を確認できません".into());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Host {
    version: u32,
    identity: String,
}

pub(super) fn host_identity() -> Result<String> {
    #[cfg(target_os = "linux")]
    {
        let machine = fs::read_to_string("/etc/machine-id")
            .map_err(|_| "安定したhost identityを確認できません")?;
        let machine = machine.trim();
        if machine.len() != 32
            || !machine.bytes().all(|byte| byte.is_ascii_hexdigit())
            || machine.bytes().all(|byte| byte == b'0')
        {
            return Err("host identityの形式が不正です".into());
        }
        Ok(format!(
            "{:x}",
            Sha256::digest(format!("codex-task-v1:{machine}:{}", unsafe {
                libc::geteuid()
            }))
        ))
    }
    #[cfg(not(target_os = "linux"))]
    Err("このOSのhost identity adapterは未対応です".into())
}

pub(super) struct Store {
    pub root: PathBuf,
    pub host: String,
}

impl Store {
    pub fn open(codex_home: &Path, key: &str) -> Result<Self> {
        private_dir(codex_home)?;
        let root = codex_home.join("worktrees");
        private_dir(&root)?;
        let host = host_identity()?;
        let _lock = Lock::acquire_short(&root.join(".task-host.lock"))?;
        let host_path = root.join(".task-host.json");
        if host_path
            .try_exists()
            .map_err(|_| "host状態を確認できません")?
        {
            let stored: Host = read_json(&host_path)?;
            if stored.version != 1 || stored.identity != host {
                return Err(
                    "別hostのtask stateを拒否しました。stateをhost間で共有しないでください".into(),
                );
            }
        } else {
            atomic_json(
                &host_path,
                &Host {
                    version: 1,
                    identity: host.clone(),
                },
            )?;
        }
        let root = root.join(key);
        private_dir(&root)?;
        let root = root.join(".tasks");
        private_dir(&root)?;
        Ok(Self { root, host })
    }

    pub fn lock(&self) -> Result<Lock> {
        Lock::acquire_short(&self.root.join("state.lock"))
    }

    pub fn path(&self, id: &str) -> Result<PathBuf> {
        if !super::valid_id(id) {
            return Err("task IDが不正です".into());
        }
        Ok(self.root.join(format!("{id}.json")))
    }

    pub fn load(&self, id: &str) -> Result<Task> {
        let task: Task = read_json(&self.path(id)?)?;
        if task.version != 1 || task.id != id || task.host != self.host {
            return Err("taskのversion・ID・所有hostが一致しません".into());
        }
        task.validate()?;
        Ok(task)
    }

    pub fn save(&self, task: &Task) -> Result<()> {
        task.validate()?;
        if task.host != self.host {
            return Err("別hostのtaskを更新できません".into());
        }
        atomic_json(&self.path(&task.id)?, task)
    }

    pub fn list(&self) -> Result<Vec<Task>> {
        let mut tasks = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|_| "task一覧を取得できません")? {
            let entry = entry.map_err(|_| "task一覧を取得できません")?;
            let name = entry.file_name();
            let name = name.to_str().ok_or("task file名が不正です")?;
            if let Some(id) = name.strip_suffix(".json")
                && super::valid_id(id)
            {
                tasks.push(self.load(id)?);
            }
        }
        tasks.sort_by(|left, right| (left.created, &left.id).cmp(&(right.created, &right.id)));
        Ok(tasks)
    }

    pub fn lease_path(&self, id: &str) -> Result<PathBuf> {
        self.path(id)?;
        Ok(self.root.join(format!("{id}.worker.lock")))
    }
}

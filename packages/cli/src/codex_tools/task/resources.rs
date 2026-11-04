//! 実行枠はtask件数ではなく、hostの利用可能資源から決める。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Result, store};

#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Budget {
    pub worker_memory_mib: u64,
    pub worker_disk_mib: u64,
    pub reserve_memory_mib: u64,
    pub minimum_disk_mib: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            worker_memory_mib: 2048,
            worker_disk_mib: 2048,
            reserve_memory_mib: 1024,
            minimum_disk_mib: 2048,
        }
    }
}

impl Budget {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join("budget.json");
        let budget: Self = if path
            .try_exists()
            .map_err(|_| "resource budgetを検査できません")?
        {
            store::read_json(&path)?
        } else {
            Self::default()
        };
        budget.validate()?;
        Ok(budget)
    }

    fn validate(&self) -> Result<()> {
        if [
            self.worker_memory_mib,
            self.worker_disk_mib,
            self.reserve_memory_mib,
            self.minimum_disk_mib,
        ]
        .iter()
        .any(|value| *value == 0 || *value > 1_048_576)
        {
            return Err("resource budgetは1〜1048576 MiBの範囲で指定してください".into());
        }
        Ok(())
    }

    pub fn capacity(&self, root: &Path) -> Result<usize> {
        self.validate()?;
        let snapshot = ResourceSnapshot::measure(root)?;
        Ok(admit(
            snapshot.cpus,
            snapshot.memory_mib,
            snapshot.disk_mib,
            self,
        ))
    }
}

const MAX_SLOTS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResourceSnapshot {
    cpus: usize,
    memory_mib: u64,
    disk_mib: u64,
}

impl ResourceSnapshot {
    fn measure(root: &Path) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt;
            let memory =
                fs::read_to_string("/proc/meminfo").map_err(|_| "利用可能RAMを確認できません")?;
            let memory_mib = memory
                .lines()
                .find_map(|line| line.strip_prefix("MemAvailable:"))
                .and_then(|line| line.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or("利用可能RAMを解析できません")?
                / 1024;
            let path = std::ffi::CString::new(root.as_os_str().as_bytes())
                .map_err(|_| "disk確認pathが不正です")?;
            let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
                return Err("利用可能diskを確認できません".into());
            }
            let stat = unsafe { stat.assume_init() };
            let disk_mib = stat.f_bavail.saturating_mul(stat.f_frsize) / (1024 * 1024);
            let cpus = std::thread::available_parallelism()
                .map(usize::from)
                .map_err(|_| "利用可能CPUを確認できません")?;
            Ok(Self {
                cpus,
                memory_mib,
                disk_mib,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = root;
            Err("このOSのresource計測adapterは未対応です".into())
        }
    }
}

fn admit(cpus: usize, available_mib: u64, disk_mib: u64, budget: &Budget) -> usize {
    if disk_mib < budget.minimum_disk_mib {
        return 0;
    }
    cpus.min(
        available_mib
            .saturating_sub(budget.reserve_memory_mib)
            .checked_div(budget.worker_memory_mib)
            .unwrap_or(0)
            .try_into()
            .unwrap_or(usize::MAX),
    )
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReservationMetadata {
    memory_mib: u64,
    disk_mib: u64,
}

impl ReservationMetadata {
    fn validate(&self) -> Result<()> {
        if [self.memory_mib, self.disk_mib]
            .iter()
            .any(|value| *value == 0 || *value > 1_048_576)
        {
            return Err("resource reservation metadataが不正です".into());
        }
        Ok(())
    }
}

/// host共通のworker資源slot。Lockを保持する間、metadata分の枠を予約する。
pub(super) struct Reservation {
    slot: usize,
    lock: store::Lock,
}

impl Reservation {
    pub fn acquire(root: &Path, budget: &Budget) -> Result<Option<Self>> {
        prepare_root(root)?;
        let _state = store::Lock::acquire_short(&root.join("state.lock"))?;
        let snapshot = ResourceSnapshot::measure(root)?;
        Self::acquire_locked(root, budget, snapshot)
    }

    #[cfg(test)]
    fn acquire_with_snapshot(
        root: &Path,
        budget: &Budget,
        snapshot: ResourceSnapshot,
    ) -> Result<Option<Self>> {
        prepare_root(root)?;
        let _state = store::Lock::acquire_short(&root.join("state.lock"))?;
        Self::acquire_locked(root, budget, snapshot)
    }

    fn acquire_locked(
        root: &Path,
        budget: &Budget,
        snapshot: ResourceSnapshot,
    ) -> Result<Option<Self>> {
        budget.validate()?;
        let cpu_slots = snapshot.cpus.min(MAX_SLOTS);
        if cpu_slots == 0 {
            return Err("利用可能CPUを確認できません".into());
        }

        let mut active = 0usize;
        let mut memory_reserved = 0u64;
        let mut disk_reserved = 0u64;
        let mut candidate: Option<(usize, store::Lock)> = None;

        // CPU数が縮小しても、以前の高slotで生存中のworkerを集計する。
        // CPU数外に存在しないslot fileは作成しない。
        for slot in 0..MAX_SLOTS {
            let path = slot_path(root, slot);
            if slot >= cpu_slots && !slot_exists(&path)? {
                continue;
            }
            match store::Lock::try_acquire(&path)? {
                Some(lock) => {
                    validate_reusable_slot(&path)?;
                    if slot < cpu_slots && candidate.is_none() {
                        candidate = Some((slot, lock));
                    }
                }
                None => {
                    let metadata: ReservationMetadata = store::read_json(&path)?;
                    metadata.validate()?;
                    active = active.saturating_add(1);
                    memory_reserved = memory_reserved.saturating_add(metadata.memory_mib);
                    disk_reserved = disk_reserved.saturating_add(metadata.disk_mib);
                }
            }
        }

        let memory_needed = budget
            .reserve_memory_mib
            .saturating_add(memory_reserved)
            .saturating_add(budget.worker_memory_mib);
        let disk_needed = budget
            .minimum_disk_mib
            .saturating_add(disk_reserved)
            .saturating_add(budget.worker_disk_mib);
        if active >= cpu_slots
            || snapshot.memory_mib < memory_needed
            || snapshot.disk_mib < disk_needed
        {
            return Ok(None);
        }
        let Some((slot, lock)) = candidate else {
            return Ok(None);
        };

        let metadata = ReservationMetadata {
            memory_mib: budget.worker_memory_mib,
            disk_mib: budget.worker_disk_mib,
        };
        write_metadata(root, &lock, &metadata)?;
        Ok(Some(Self { slot, lock }))
    }

    pub fn slot(&self) -> usize {
        self.slot
    }

    pub fn fd(&self) -> i32 {
        #[cfg(unix)]
        {
            self.lock.fd()
        }
        #[cfg(not(unix))]
        {
            -1
        }
    }

    pub fn inherit(&self) -> Result<()> {
        self.lock.inherit()
    }

    pub fn from_inherited(root: &Path, slot: usize, fd: i32) -> Result<Self> {
        prepare_root(root)?;
        if slot >= MAX_SLOTS {
            return Err("resource reservation slotが不正です".into());
        }
        #[cfg(unix)]
        let lock = unsafe { store::Lock::from_inherited(fd, &slot_path(root, slot))? };
        #[cfg(not(unix))]
        let lock = {
            let _ = fd;
            return Err("このOSのresource reservation継承は未対応です".into());
        };
        let metadata: ReservationMetadata = store::read_json(&slot_path(root, slot))?;
        metadata.validate()?;
        Ok(Self { slot, lock })
    }
}

fn prepare_root(root: &Path) -> Result<()> {
    if let Some(parent) = root.parent()
        && matches!(
            fs::symlink_metadata(parent),
            Err(ref error) if error.kind() == std::io::ErrorKind::NotFound
        )
    {
        store::private_dir(parent)?;
    }
    store::private_dir(root)
}

fn slot_path(root: &Path, slot: usize) -> PathBuf {
    root.join(format!("slot-{slot}.json"))
}

fn slot_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("resource reservation slotを確認できません".into()),
    }
}

fn validate_reusable_slot(path: &Path) -> Result<()> {
    // lockを取れた固定slotはhelper管理の使い捨てmetadata。前のcoordinatorが
    // write/sync途中で落ちた場合も、private fileのidentityを確認して同inodeを再利用する。
    // active slotはこの関数を通らず、read_jsonとmetadata validationを必須にする。
    store::open_private(path, false).map(|_| ())
}

fn write_metadata(root: &Path, lock: &store::Lock, metadata: &ReservationMetadata) -> Result<()> {
    let bytes = serde_json::to_vec(metadata).map_err(|_| "resource reservationを生成できません")?;
    #[cfg(unix)]
    {
        let fd = lock.fd();
        if unsafe { libc::ftruncate(fd, 0) } != 0
            || unsafe { libc::lseek(fd, 0, libc::SEEK_SET) } < 0
        {
            return Err("resource reservationを更新できません".into());
        }
        let mut written = 0usize;
        while written < bytes.len() {
            let result =
                unsafe { libc::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written) };
            if result < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err("resource reservationを書き込めません".into());
            }
            if result == 0 {
                return Err("resource reservationを書き込めません".into());
            }
            written += result as usize;
        }
        if unsafe { libc::fsync(fd) } != 0 {
            return Err("resource reservationを同期できません".into());
        }
    }
    #[cfg(not(unix))]
    return Err("このOSのresource reservationは未対応です".into());
    fs::File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "resource reservation directoryを同期できません".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "task-admission-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        store::private_dir(&path).unwrap();
        path
    }

    fn budget(memory_mib: u64, disk_mib: u64, reserve_mib: u64, minimum_disk_mib: u64) -> Budget {
        Budget {
            worker_memory_mib: memory_mib,
            worker_disk_mib: disk_mib,
            reserve_memory_mib: reserve_mib,
            minimum_disk_mib,
        }
    }

    fn snapshot(cpus: usize, memory_mib: u64, disk_mib: u64) -> ResourceSnapshot {
        ResourceSnapshot {
            cpus,
            memory_mib,
            disk_mib,
        }
    }

    fn simultaneous_acquires(
        root: &Path,
        budget: &Budget,
        measured: ResourceSnapshot,
    ) -> Vec<Option<Reservation>> {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let root = root.to_path_buf();
                let barrier = barrier.clone();
                let budget = Budget {
                    worker_memory_mib: budget.worker_memory_mib,
                    worker_disk_mib: budget.worker_disk_mib,
                    reserve_memory_mib: budget.reserve_memory_mib,
                    minimum_disk_mib: budget.minimum_disk_mib,
                };
                std::thread::spawn(move || {
                    barrier.wait();
                    Reservation::acquire_with_snapshot(&root, &budget, measured)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap().unwrap())
            .collect()
    }

    #[test]
    fn admission_tracks_resources_without_limiting_task_registration() {
        let budget = Budget::default();
        assert_eq!(admit(16, 1024, 4096, &budget), 0);
        assert_eq!(admit(16, 5120, 4096, &budget), 2);
        assert_eq!(admit(16, 5120, 1024, &budget), 0);
        assert_eq!(admit(2, 65536, 4096, &budget), 2);
        assert_eq!(Budget::default().worker_disk_mib, 2048);
        let legacy: Budget = serde_json::from_value(serde_json::json!({
            "worker_memory_mib":2048,
            "reserve_memory_mib":1024,
            "minimum_disk_mib":2048
        }))
        .unwrap();
        assert_eq!(legacy.worker_disk_mib, 2048);
    }

    #[cfg(unix)]
    #[test]
    fn reservations_prevent_cpu_oversubscription_across_repositories_and_release() {
        let root = test_root();
        let budget = budget(2, 2, 1, 1);
        let measured = snapshot(1, 100, 100);
        let reservations = simultaneous_acquires(&root, &budget, measured);
        assert_eq!(reservations.iter().filter(|item| item.is_some()).count(), 1);
        let first = reservations.into_iter().flatten().next().unwrap();
        assert_eq!(first.slot(), 0);
        let slot_path = slot_path(&root, 0);
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(&slot_path).unwrap().ino()
        };

        // 別repositoryの同時workerも同じhost slotへ予約する。
        assert!(
            Reservation::acquire_with_snapshot(&root, &budget, measured)
                .unwrap()
                .is_none()
        );
        drop(first);

        let second = Reservation::acquire_with_snapshot(&root, &budget, measured)
            .unwrap()
            .unwrap();
        assert_eq!(second.slot(), 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(&slot_path).unwrap().ino(), inode);
        }
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn reservations_prevent_ram_oversubscription_across_repositories() {
        let root = test_root();
        let budget = budget(4, 2, 1, 1);
        let measured = snapshot(3, 8, 100);
        let reservations = simultaneous_acquires(&root, &budget, measured);
        assert_eq!(reservations.iter().filter(|item| item.is_some()).count(), 1);
        drop(reservations);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn reservations_prevent_disk_oversubscription_across_repositories() {
        let root = test_root();
        let budget = budget(2, 4, 1, 1);
        let measured = snapshot(3, 100, 8);
        let reservations = simultaneous_acquires(&root, &budget, measured);
        assert_eq!(reservations.iter().filter(|item| item.is_some()).count(), 1);
        drop(reservations);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn reservations_reject_symlink_and_malformed_slot_metadata() {
        use std::os::unix::fs::symlink;

        let root = test_root();
        let budget = budget(2, 2, 1, 1);
        let measured = snapshot(1, 100, 100);
        let target = root.join("target");
        fs::write(&target, b"private target").unwrap();
        symlink(&target, slot_path(&root, 0)).unwrap();
        assert!(Reservation::acquire_with_snapshot(&root, &budget, measured).is_err());
        fs::remove_dir_all(&root).unwrap();

        let root = test_root();
        let path = slot_path(&root, 0);
        let active_lock = store::Lock::try_acquire(&path).unwrap().unwrap();
        let mut malformed = store::open_private(&path, true).unwrap();
        use std::io::Write;
        malformed.write_all(b"{not-json").unwrap();
        malformed.sync_all().unwrap();
        drop(malformed);
        assert!(Reservation::acquire_with_snapshot(&root, &budget, measured).is_err());
        drop(active_lock);
        // Helper管理slotのlockが解放されれば、途中書き込みmetadataを同inodeで回復する。
        let recovered = Reservation::acquire_with_snapshot(&root, &budget, measured)
            .unwrap()
            .unwrap();
        drop(recovered);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn inherited_reservation_keeps_slot_until_child_releases_same_inode_lock() {
        let root = test_root();
        let budget = budget(2, 2, 1, 1);
        let measured = snapshot(1, 100, 100);
        let reservation = Reservation::acquire_with_snapshot(&root, &budget, measured)
            .unwrap()
            .unwrap();
        reservation.inherit().unwrap();
        drop(
            store::Lock::try_acquire(&slot_path(&root, 1))
                .unwrap()
                .unwrap(),
        );
        let wrong_slot_fd = unsafe { libc::dup(reservation.fd()) };
        assert!(wrong_slot_fd >= 3);
        assert!(Reservation::from_inherited(&root, 1, wrong_slot_fd).is_err());
        let inherited_fd = unsafe { libc::dup(reservation.fd()) };
        assert!(inherited_fd >= 3);
        let child = Reservation::from_inherited(&root, reservation.slot(), inherited_fd).unwrap();
        drop(reservation);
        assert!(
            Reservation::acquire_with_snapshot(&root, &budget, measured)
                .unwrap()
                .is_none()
        );
        drop(child);
        assert!(
            Reservation::acquire_with_snapshot(&root, &budget, measured)
                .unwrap()
                .is_some()
        );
        fs::remove_dir_all(root).unwrap();
    }
}

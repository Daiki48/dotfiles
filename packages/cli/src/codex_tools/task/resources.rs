//! 実行枠はtask件数ではなく、hostの利用可能資源から決める。

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Result, store};

#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Budget {
    pub worker_memory_mib: u64,
    pub reserve_memory_mib: u64,
    pub minimum_disk_mib: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            worker_memory_mib: 2048,
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
        if budget.worker_memory_mib == 0
            || budget.reserve_memory_mib == 0
            || budget.minimum_disk_mib == 0
            || budget.worker_memory_mib > 1_048_576
            || budget.reserve_memory_mib > 1_048_576
            || budget.minimum_disk_mib > 1_048_576
        {
            return Err("resource budgetは1〜1048576 MiBの範囲で指定してください".into());
        }
        Ok(budget)
    }

    pub fn capacity(&self, root: &Path) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt;
            let memory =
                fs::read_to_string("/proc/meminfo").map_err(|_| "利用可能RAMを確認できません")?;
            let memory = memory
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
            let disk = stat.f_bavail.saturating_mul(stat.f_frsize) / (1024 * 1024);
            let cpus = std::thread::available_parallelism()
                .map(usize::from)
                .map_err(|_| "利用可能CPUを確認できません")?;
            Ok(admit(cpus, memory, disk, self))
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admission_tracks_resources_without_limiting_task_registration() {
        let budget = Budget::default();
        assert_eq!(admit(16, 1024, 4096, &budget), 0);
        assert_eq!(admit(16, 5120, 4096, &budget), 2);
        assert_eq!(admit(16, 5120, 1024, &budget), 0);
        assert_eq!(admit(2, 65536, 4096, &budget), 2);
    }
}

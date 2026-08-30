//! Generic orchestration for project-owned self-hosted runner storage cleanup.
//!
//! The adapter remains the deletion safety boundary: it owns the controller,
//! VM, recovery-state, canonical-image, and lock checks. This helper discovers
//! only explicitly registered adapters, validates their reports, audits by
//! default, and requires an exact target ID for apply.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use serde::{Deserialize, Serialize};

use super::{process, trust};

const SCHEMA_VERSION: u64 = 1;
const DEFAULT_CONFIG_RELATIVE: &str = "runner-storage-cleanup/targets.d";
const MAX_MANIFESTS: usize = 128;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_ARGUMENTS: usize = 64;
const MAX_ARGUMENT_BYTES: usize = 4096;
const MAX_ID_BYTES: usize = 64;
const MAX_CANDIDATES: usize = 4096;
const MAX_REPORT_BYTES: usize = 1024 * 1024;
const ADAPTER_TIMEOUT: Duration = Duration::from_secs(5 * 60 + 10);
const ADAPTER_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetManifest {
    schema_version: u64,
    id: String,
    working_directory: PathBuf,
    storage_root: PathBuf,
    command: CommandSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandSpec {
    program: PathBuf,
    args: Vec<String>,
    apply_args: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct CandidateReport {
    kind: String,
    path: PathBuf,
    recovery_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AdapterReport {
    applied: bool,
    candidate_count: usize,
    reclaimable_bytes: u64,
    candidates: Vec<CandidateReport>,
}

impl AdapterReport {
    pub(crate) fn candidate_count(&self) -> usize {
        self.candidate_count
    }

    pub(crate) fn reclaimable_bytes(&self) -> u64 {
        self.reclaimable_bytes
    }
}

#[derive(Debug, Serialize)]
struct TargetReport {
    id: String,
    report: AdapterReport,
}

#[derive(Debug, Serialize)]
struct AuditOutput {
    action: &'static str,
    targets: Vec<TargetReport>,
}

#[derive(Debug, Serialize)]
struct ApplyOutput {
    action: &'static str,
    target: String,
    audit: AdapterReport,
    applied: AdapterReport,
    post_audit: AdapterReport,
}

#[derive(Debug, Eq, PartialEq)]
enum Action {
    Audit { target: Option<String> },
    Apply { target: String },
}

#[derive(Debug)]
struct Arguments {
    action: Action,
    config_dir: PathBuf,
}

fn usage() -> &'static str {
    "usage:\n  runner-storage-cleanup audit [--target <id>] [--config-dir <absolute-path>]\n  runner-storage-cleanup apply --target <id> [--config-dir <absolute-path>]\n\nAudit is read-only. Apply never stops a controller and succeeds only when the registered adapter proves its own maintenance gates."
}

fn valid_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_ID_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(byte))
}

pub(crate) fn default_config_dir() -> Result<PathBuf, String> {
    if let Some(value) = env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err("XDG_CONFIG_HOME must be an absolute path".to_string());
        }
        return Ok(path.join(DEFAULT_CONFIG_RELATIVE));
    }
    home::home_dir()
        .map(|home| home.join(".config").join(DEFAULT_CONFIG_RELATIVE))
        .ok_or_else(|| "cannot determine the configuration directory".to_string())
}

fn parse_args<I>(args: I) -> Result<Arguments, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut values = args.into_iter();
    let command = values
        .next()
        .ok_or_else(|| usage().to_string())?
        .into_string()
        .map_err(|_| "command must be valid UTF-8".to_string())?;
    if command == "--help" || command == "-h" {
        return Err(usage().to_string());
    }
    if command != "audit" && command != "apply" {
        return Err(format!("unknown command: {command}\n{}", usage()));
    }

    let mut target = None;
    let mut config_dir = None;
    while let Some(raw) = values.next() {
        let flag = raw
            .into_string()
            .map_err(|_| "option must be valid UTF-8".to_string())?;
        match flag.as_str() {
            "--target" => {
                if target.is_some() {
                    return Err("--target may be specified only once".to_string());
                }
                let value = values
                    .next()
                    .ok_or_else(|| "--target requires a value".to_string())?
                    .into_string()
                    .map_err(|_| "target ID must be valid UTF-8".to_string())?;
                if !valid_id(&value) {
                    return Err("target ID is invalid".to_string());
                }
                target = Some(value);
            }
            "--config-dir" => {
                if config_dir.is_some() {
                    return Err("--config-dir may be specified only once".to_string());
                }
                let value = PathBuf::from(
                    values
                        .next()
                        .ok_or_else(|| "--config-dir requires a value".to_string())?,
                );
                if !value.is_absolute() {
                    return Err("--config-dir must be an absolute path".to_string());
                }
                config_dir = Some(value);
            }
            _ => return Err(format!("unknown option: {flag}")),
        }
    }

    let action = if command == "audit" {
        Action::Audit { target }
    } else {
        Action::Apply {
            target: target.ok_or_else(|| "apply requires --target <id>".to_string())?,
        }
    };
    Ok(Arguments {
        action,
        config_dir: config_dir.unwrap_or(default_config_dir()?),
    })
}

fn reject_non_normal_path(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(format!(
            "{label} must be a normalized absolute path: {}",
            path.display()
        ));
    }
    Ok(())
}

fn current_uid() -> u32 {
    #[cfg(unix)]
    // SAFETY: getuid has no pointer arguments and reads only process identity.
    unsafe {
        libc::getuid()
    }
    #[cfg(not(unix))]
    0
}

fn validate_user_directory(path: &Path, label: &str) -> Result<(), String> {
    reject_non_normal_path(path, label)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {label} {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "{label} is not a regular directory: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    if metadata.uid() != current_uid() || metadata.mode() & 0o022 != 0 {
        return Err(format!(
            "{label} has an unsafe owner or mode: {}",
            path.display()
        ));
    }
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve {label} {}: {error}", path.display()))?;
    if canonical != path {
        return Err(format!(
            "{label} contains a symlink or alias: {}",
            path.display()
        ));
    }
    Ok(())
}

fn validate_manifest_file(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect manifest {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_MANIFEST_BYTES
    {
        return Err(format!(
            "manifest is not a bounded regular file: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    if metadata.uid() != current_uid() || metadata.mode() & 0o022 != 0 || metadata.nlink() != 1 {
        return Err(format!(
            "manifest has an unsafe owner, mode, or link count: {}",
            path.display()
        ));
    }
    Ok(())
}

fn validate_arguments(values: &[String], label: &str, allow_empty: bool) -> Result<(), String> {
    if (!allow_empty && values.is_empty()) || values.len() > MAX_ARGUMENTS {
        return Err(format!("{label} has an invalid argument count"));
    }
    for value in values {
        if value.is_empty() || value.len() > MAX_ARGUMENT_BYTES || value.contains('\0') {
            return Err(format!("{label} contains an invalid argument"));
        }
    }
    Ok(())
}

fn validate_manifest(path: &Path, manifest: &TargetManifest) -> Result<(), String> {
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(format!("unsupported manifest schema in {}", path.display()));
    }
    if !valid_id(&manifest.id)
        || path.file_stem().and_then(|value| value.to_str()) != Some(manifest.id.as_str())
    {
        return Err(format!(
            "manifest filename and target ID do not match: {}",
            path.display()
        ));
    }
    validate_user_directory(&manifest.working_directory, "working directory")?;
    validate_user_directory(&manifest.storage_root, "storage root")?;
    reject_non_normal_path(&manifest.command.program, "adapter program")?;
    let program = manifest
        .command
        .program
        .to_str()
        .ok_or_else(|| "adapter program must be valid UTF-8".to_string())?;
    trust::trusted_system_binary(program, "runner cleanup adapter")?;
    validate_arguments(&manifest.command.args, "adapter args", true)?;
    validate_arguments(&manifest.command.apply_args, "adapter apply_args", false)?;
    Ok(())
}

fn load_manifests(config_dir: &Path) -> Result<BTreeMap<String, TargetManifest>, String> {
    validate_user_directory(config_dir, "configuration directory")?;
    let mut paths = Vec::new();
    for (index, entry) in fs::read_dir(config_dir)
        .map_err(|error| format!("cannot read configuration directory: {error}"))?
        .enumerate()
    {
        if index >= MAX_MANIFESTS {
            return Err("registered target count exceeds the safety limit".to_string());
        }
        let entry = entry.map_err(|error| format!("cannot read configuration entry: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            return Err(format!(
                "unexpected configuration entry: {}",
                path.display()
            ));
        }
        validate_manifest_file(&path)?;
        paths.push(path);
    }
    paths.sort();
    if paths.is_empty() {
        return Err("no runner storage cleanup targets are registered".to_string());
    }

    let mut manifests = BTreeMap::new();
    for path in paths {
        let bytes = fs::read(&path)
            .map_err(|error| format!("cannot read manifest {}: {error}", path.display()))?;
        let manifest: TargetManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid manifest {}: {error}", path.display()))?;
        validate_manifest(&path, &manifest)?;
        if manifests.insert(manifest.id.clone(), manifest).is_some() {
            return Err("duplicate runner storage cleanup target ID".to_string());
        }
    }
    Ok(manifests)
}

pub(crate) fn audit_registered(config_dir: &Path) -> Result<Vec<(String, AdapterReport)>, String> {
    let manifests = load_manifests(config_dir)?;
    manifests
        .iter()
        .map(|(id, manifest)| run_adapter(manifest, false).map(|report| (id.clone(), report)))
        .collect()
}

pub(crate) fn apply_registered(
    config_dir: &Path,
    target: &str,
    expected: &AdapterReport,
) -> Result<(AdapterReport, AdapterReport, AdapterReport), String> {
    let manifests = load_manifests(config_dir)?;
    let manifest = manifests
        .get(target)
        .ok_or_else(|| format!("unknown target ID: {target}"))?;
    let audit = run_adapter(manifest, false)?;
    if &audit != expected {
        return Err("adapter audit changed after interactive confirmation".to_string());
    }
    let applied = run_adapter(manifest, true)?;
    if audit.candidates != applied.candidates
        || audit.candidate_count != applied.candidate_count
        || audit.reclaimable_bytes != applied.reclaimable_bytes
    {
        return Err("adapter apply report differs from the pre-apply audit".to_string());
    }
    let post_audit = run_adapter(manifest, false)?;
    if post_audit.candidate_count != 0 || post_audit.reclaimable_bytes != 0 {
        return Err("adapter still reports cleanup candidates after apply".to_string());
    }
    Ok((audit, applied, post_audit))
}

fn validate_report(
    manifest: &TargetManifest,
    report: &AdapterReport,
    apply: bool,
) -> Result<(), String> {
    if report.applied != apply {
        return Err("adapter report applied state does not match the request".to_string());
    }
    if report.candidate_count != report.candidates.len() || report.candidates.len() > MAX_CANDIDATES
    {
        return Err("adapter report candidate count is invalid".to_string());
    }
    let mut identities = BTreeSet::new();
    for candidate in &report.candidates {
        if candidate.kind.is_empty()
            || candidate.kind.len() > MAX_ID_BYTES
            || candidate.recovery_key.is_empty()
            || candidate.recovery_key.len() > MAX_ARGUMENT_BYTES
        {
            return Err("adapter report contains an invalid candidate identity".to_string());
        }
        reject_non_normal_path(&candidate.path, "cleanup candidate")?;
        if !candidate.path.starts_with(&manifest.storage_root) {
            return Err(format!(
                "cleanup candidate escapes the registered storage root: {}",
                candidate.path.display()
            ));
        }
        if !identities.insert((
            candidate.path.clone(),
            candidate.kind.clone(),
            candidate.recovery_key.clone(),
        )) {
            return Err("adapter report contains duplicate candidates".to_string());
        }
    }
    Ok(())
}

fn run_adapter(manifest: &TargetManifest, apply: bool) -> Result<AdapterReport, String> {
    let program = manifest
        .command
        .program
        .to_str()
        .ok_or_else(|| "adapter program must be valid UTF-8".to_string())?;
    let trusted_program = trust::trusted_system_binary(program, "runner cleanup adapter")?;
    let mut command = Command::new(trusted_program);
    process::clear_environment(&mut command);
    command
        .current_dir(&manifest.working_directory)
        .env(
            "HOME",
            home::home_dir().ok_or_else(|| "cannot determine HOME".to_string())?,
        )
        .env("PATH", ADAPTER_PATH)
        .env("LANG", "C.UTF-8")
        .args(&manifest.command.args);
    if apply {
        command.args(&manifest.command.apply_args);
    }
    let output = process::run_host_with_limit(&mut command, ADAPTER_TIMEOUT, MAX_REPORT_BYTES)
        .map_err(|error| format!("adapter process failed for {}: {error}", manifest.id))?;
    if !output.status.success() {
        return Err(format!(
            "adapter rejected {} for {} (exit {:?}): {}",
            if apply { "apply" } else { "audit" },
            manifest.id,
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let report: AdapterReport = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("adapter returned invalid JSON for {}: {error}", manifest.id))?;
    validate_report(manifest, &report, apply)?;
    Ok(report)
}

fn audit(manifests: &BTreeMap<String, TargetManifest>, target: Option<&str>) -> Result<(), String> {
    let selected = if let Some(id) = target {
        vec![(
            id,
            manifests
                .get(id)
                .ok_or_else(|| format!("unknown target ID: {id}"))?,
        )]
    } else {
        manifests
            .iter()
            .map(|(id, manifest)| (id.as_str(), manifest))
            .collect()
    };
    let mut targets = Vec::with_capacity(selected.len());
    for (id, manifest) in selected {
        targets.push(TargetReport {
            id: id.to_string(),
            report: run_adapter(manifest, false)?,
        });
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&AuditOutput {
            action: "audit",
            targets,
        })
        .map_err(|error| format!("cannot serialize audit result: {error}"))?
    );
    Ok(())
}

fn apply(manifests: &BTreeMap<String, TargetManifest>, target: &str) -> Result<(), String> {
    let manifest = manifests
        .get(target)
        .ok_or_else(|| format!("unknown target ID: {target}"))?;
    let audit = run_adapter(manifest, false)?;
    let applied = run_adapter(manifest, true)?;
    if audit.candidates != applied.candidates
        || audit.candidate_count != applied.candidate_count
        || audit.reclaimable_bytes != applied.reclaimable_bytes
    {
        return Err("adapter apply report differs from the pre-apply audit".to_string());
    }
    let post_audit = run_adapter(manifest, false)?;
    if post_audit.candidate_count != 0 || post_audit.reclaimable_bytes != 0 {
        return Err("adapter still reports cleanup candidates after apply".to_string());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&ApplyOutput {
            action: "apply",
            target: target.to_string(),
            audit,
            applied,
            post_audit,
        })
        .map_err(|error| format!("cannot serialize apply result: {error}"))?
    );
    Ok(())
}

fn run<I>(args: I) -> Result<(), String>
where
    I: IntoIterator<Item = OsString>,
{
    let arguments = parse_args(args)?;
    let manifests = load_manifests(&arguments.config_dir)?;
    match arguments.action {
        Action::Audit { target } => audit(&manifests, target.as_deref()),
        Action::Apply { target } => apply(&manifests, &target),
    }
}

pub(crate) fn entrypoint<I>(args: I) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    match run(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("runner-storage-cleanup: {error}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        config: PathBuf,
        repository: PathBuf,
        storage: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let root = env::temp_dir().join(format!(
                "runner-storage-cleanup-{name}-{suffix}-{}",
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let config = root.join("config");
            let repository = root.join("repository");
            let storage = root.join("storage");
            fs::create_dir(&root).unwrap();
            fs::create_dir(&config).unwrap();
            fs::create_dir(&repository).unwrap();
            fs::create_dir(&storage).unwrap();
            Self {
                root,
                config,
                repository,
                storage,
            }
        }

        fn manifest(&self, id: &str, program: &str, args: &[&str], apply_args: &[&str]) {
            let value = serde_json::json!({
                "schema_version": 1,
                "id": id,
                "working_directory": self.repository,
                "storage_root": self.storage,
                "command": {
                    "program": program,
                    "args": args,
                    "apply_args": apply_args,
                }
            });
            fs::write(
                self.config.join(format!("{id}.json")),
                serde_json::to_vec_pretty(&value).unwrap(),
            )
            .unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn parser_defaults_to_audit_and_requires_an_exact_apply_target() {
        let config = PathBuf::from("/tmp/runner-config");
        assert_eq!(
            parse_args([
                OsString::from("audit"),
                OsString::from("--config-dir"),
                config.clone().into_os_string(),
            ])
            .unwrap()
            .action,
            Action::Audit { target: None }
        );
        assert!(parse_args([OsString::from("apply")]).is_err());
        assert!(
            parse_args([
                OsString::from("apply"),
                OsString::from("--target"),
                OsString::from("../unsafe"),
            ])
            .is_err()
        );
    }

    #[test]
    fn manifests_reject_unknown_fields_symlinks_and_mismatched_ids() {
        let fixture = Fixture::new("manifest-validation");
        fixture.manifest("alpha", "/usr/bin/true", &[], &["--apply"]);
        let path = fixture.config.join("alpha.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["unexpected"] = serde_json::Value::Bool(true);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(load_manifests(&fixture.config).is_err());

        fs::remove_file(&path).unwrap();
        fixture.manifest("beta", "/usr/bin/true", &[], &["--apply"]);
        fs::rename(
            fixture.config.join("beta.json"),
            fixture.config.join("other.json"),
        )
        .unwrap();
        assert!(load_manifests(&fixture.config).is_err());
    }

    #[test]
    fn report_rejects_candidates_outside_the_registered_storage() {
        let fixture = Fixture::new("report-boundary");
        fixture.manifest("alpha", "/usr/bin/true", &[], &["--apply"]);
        let manifests = load_manifests(&fixture.config).unwrap();
        let manifest = manifests.get("alpha").unwrap();
        let report = AdapterReport {
            applied: false,
            candidate_count: 1,
            reclaimable_bytes: 1,
            candidates: vec![CandidateReport {
                kind: "cache".to_string(),
                path: fixture.root.join("outside"),
                recovery_key: "complete".to_string(),
            }],
        };
        assert!(validate_report(manifest, &report, false).is_err());
    }

    #[test]
    fn manifest_rejects_group_writable_configuration() {
        let fixture = Fixture::new("config-mode");
        fixture.manifest("alpha", "/usr/bin/true", &[], &["--apply"]);
        fs::set_permissions(&fixture.config, fs::Permissions::from_mode(0o775)).unwrap();
        assert!(load_manifests(&fixture.config).is_err());
    }

    #[test]
    fn registered_apply_is_bound_to_the_confirmed_audit_and_rechecks_empty_state() {
        let fixture = Fixture::new("apply-protocol");
        let candidate = fixture.storage.join("candidate.qcow2");
        fs::write(&candidate, b"candidate").unwrap();
        let script = r#"candidate="$1"
apply=false
if [ "${2:-}" = --apply ]; then apply=true; fi
if [ -e "$candidate" ]; then
  printf '{"applied":%s,"candidate_count":1,"reclaimable_bytes":9,"candidates":[{"kind":"candidate-b","path":"%s","recovery_key":"complete"}]}\n' "$apply" "$candidate"
  if [ "$apply" = true ]; then rm -- "$candidate"; fi
else
  printf '{"applied":%s,"candidate_count":0,"reclaimable_bytes":0,"candidates":[]}\n' "$apply"
fi"#;
        fixture.manifest(
            "alpha",
            "/usr/bin/bash",
            &["-c", script, "adapter", candidate.to_str().unwrap()],
            &["--apply"],
        );
        let audit = audit_registered(&fixture.config).unwrap().remove(0).1;
        let (_, applied, post) = apply_registered(&fixture.config, "alpha", &audit).unwrap();
        assert!(applied.applied);
        assert_eq!(post.candidate_count, 0);
        assert!(!candidate.exists());
    }
}

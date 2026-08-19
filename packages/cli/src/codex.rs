use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(unix)]
use std::{
    ffi::{CStr, OsStr},
    os::unix::ffi::OsStrExt,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::utils::{create_symlink, run_command};

const CODEX_FILES: &[(&str, &str)] = &[
    (".codex/AGENTS.md", ".codex/AGENTS.md"),
    (".codex/rules/default.rules", ".codex/rules/default.rules"),
];
const MANAGED_HOOK_SOURCE: &str = ".codex/hooks/block_git_write.py";
const MANAGED_HOOK_DESTINATION: &str = ".codex/hooks/block_git_write.py";
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ManagedInstallMode {
    PreserveSource,
    OwnerExecutable,
}

const MANAGED_HELPERS: &[(&str, &str, ManagedInstallMode)] = &[
    (
        ".codex/helpers/codex-worktree",
        ".local/bin/codex-worktree",
        ManagedInstallMode::PreserveSource,
    ),
    (
        ".codex/helpers/codex-delivery",
        ".local/bin/codex-delivery",
        ManagedInstallMode::OwnerExecutable,
    ),
];
const MANAGED_HOOK_STATE_SUFFIX: &str = ".managed.sha256";

// Skill は Codex と他の対応エージェントで共有できる標準パスへ配置する。
// ディレクトリごとリンクすることで、Skill の追加時に CLI 側の列挙を更新せずに済む。
const CODEX_DIRS: &[(&str, &str)] = &[(".agents/skills", ".agents/skills")];

// config.tomlはproject trustやhook trustなどの端末固有値を保持するため、
// symlinkせず、共有するtop-level keyとagents keyだけをsetup時に移行する。
const CODEX_CONFIG_TEMPLATE: &str = ".codex/config.base.toml";
const MANAGED_CONFIG_KEYS: &[&str] = &[
    "model",
    "model_reasoning_effort",
    "plan_mode_reasoning_effort",
    "approval_policy",
    "approvals_reviewer",
    "sandbox_mode",
    "commit_attribution",
];
const MANAGED_AGENT_KEYS: &[&str] = &[
    "enabled",
    "max_concurrent_threads_per_session",
    "default_subagent_model",
    "default_subagent_reasoning_effort",
];
const MANAGED_HOOK_COMMAND: &str = "command = 'python3 \"$HOME/.codex/hooks/block_git_write.py\"'";
const RETIRED_HOOK_COMMAND: &str =
    "command = 'python3 \"$HOME/.codex/hooks/prevent_irreversible_git.py\"'";
const MANAGED_HOOK_CONFIG: &str = r#"[[hooks.PreToolUse]]
matcher = "^Bash$"

[[hooks.PreToolUse.hooks]]
type = "command"
command = 'python3 "$HOME/.codex/hooks/block_git_write.py"'
timeout = 10
statusMessage = "Git/GitHub操作を確認中""#;
const PRE_TOOL_USE_HEADER: &str = "[[hooks.PreToolUse]]";
const PRE_TOOL_USE_HOOK_HEADER: &str = "[[hooks.PreToolUse.hooks]]";

fn is_codex_installed() -> bool {
    Command::new("codex")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn codex_install() -> Result<()> {
    println!("Installing Codex CLI via npm...");
    let mut cmd = Command::new("npm");
    cmd.args(["install", "-g", "@openai/codex"]);
    run_command(cmd, "Failed to install Codex CLI")
}

fn codex_check() -> Result<()> {
    println!("\nCodex CLI version:");
    match Command::new("codex").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Failed to get version info.");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => eprintln!("Failed to execute codex: {}", e),
    }
    Ok(())
}

fn contains_legacy_profile_config(contents: &str) -> bool {
    contents.lines().any(|line| {
        let trimmed = line.trim_start();
        let is_profile_selector = trimmed
            .strip_prefix("profile")
            .is_some_and(|rest| rest.trim_start().starts_with('='));
        let is_profile_table =
            trimmed.starts_with("[profiles.") || trimmed.starts_with("[profiles]");

        !trimmed.starts_with('#') && (is_profile_selector || is_profile_table)
    })
}

fn copy_file_exclusive(source: &Path, destination: &Path) -> Result<()> {
    let permissions = fs::metadata(source)
        .with_context(|| format!("Failed to inspect permissions for {}", source.display()))?
        .permissions();
    copy_file_exclusive_with_permissions(source, destination, permissions)
}

fn copy_file_exclusive_with_permissions(
    source: &Path,
    destination: &Path,
    permissions: fs::Permissions,
) -> Result<()> {
    let mut destination_created = false;
    let copy_result = (|| {
        let mut input = fs::File::open(source)
            .with_context(|| format!("Failed to open {}", source.display()))?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut output = options
            .open(destination)
            .with_context(|| format!("Failed to create {}", destination.display()))?;
        destination_created = true;
        std::io::copy(&mut input, &mut output).with_context(|| {
            format!(
                "Failed to copy from {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        fs::set_permissions(destination, permissions)
            .with_context(|| format!("Failed to set permissions on {}", destination.display()))?;
        output
            .sync_all()
            .with_context(|| format!("Failed to sync {}", destination.display()))?;
        Ok(())
    })();
    if let Err(error) = copy_result {
        if destination_created {
            let _ = fs::remove_file(destination);
        }
        return Err(error);
    }
    Ok(())
}

fn write_file_exclusive(
    destination: &Path,
    contents: &str,
    permissions: fs::Permissions,
) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut output = options
        .open(destination)
        .with_context(|| format!("Failed to create {}", destination.display()))?;
    output
        .write_all(contents.as_bytes())
        .with_context(|| format!("Failed to write {}", destination.display()))?;
    fs::set_permissions(destination, permissions)
        .with_context(|| format!("Failed to set permissions on {}", destination.display()))?;
    output
        .sync_all()
        .with_context(|| format!("Failed to sync {}", destination.display()))?;
    Ok(())
}

fn verify_managed_symlink(expected: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(destination)
        .with_context(|| format!("Failed to inspect managed path {}", destination.display()))?;
    if !metadata.file_type().is_symlink() {
        anyhow::bail!(
            "Managed path {} must be a symlink to {}",
            destination.display(),
            expected.display()
        );
    }
    let expected = fs::canonicalize(expected)
        .with_context(|| format!("Failed to resolve managed source {}", expected.display()))?;
    let actual = fs::canonicalize(destination).with_context(|| {
        format!(
            "Failed to resolve managed symlink {}",
            destination.display()
        )
    })?;
    if actual != expected {
        anyhow::bail!(
            "Managed symlink {} points to {}, expected {}",
            destination.display(),
            actual.display(),
            expected.display()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink_exclusive(source: &Path, destination: &Path) -> Result<()> {
    use std::os::unix::fs::symlink;

    let target = fs::read_link(source)
        .with_context(|| format!("Failed to read symlink {}", source.display()))?;
    symlink(&target, destination).with_context(|| {
        format!(
            "Failed to archive symlink {} to {}",
            source.display(),
            destination.display()
        )
    })
}

#[cfg(not(unix))]
fn copy_symlink_exclusive(_source: &Path, _destination: &Path) -> Result<()> {
    anyhow::bail!("Codex config symlink migration is supported only on Unix")
}

fn ensure_shared_symlink(source: &str, destination: &str) -> Result<()> {
    create_symlink(source, destination)?;
    let dotfiles_path = std::env::current_dir().context("Failed to get current directory")?;
    let home = home::home_dir().context("Cannot find home directory")?;
    verify_managed_symlink(&dotfiles_path.join(source), &home.join(destination))
}

fn is_assignment_for(line: &str, keys: &[&str]) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with('[') {
        return false;
    }
    keys.iter().any(|key| {
        trimmed
            .strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    })
}

fn table_header(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let header = trimmed
        .split_once('#')
        .map_or(trimmed, |(before, _)| before)
        .trim_end();
    header.starts_with('[').then_some(header)
}

fn managed_assignments(template: &str, section: Option<&str>, keys: &[&str]) -> Vec<String> {
    let mut current_section = None;
    template
        .lines()
        .filter_map(|line| {
            if let Some(header) = table_header(line) {
                current_section = Some(header);
                return None;
            }
            if current_section == section && is_assignment_for(line, keys) {
                Some(line.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn remove_retired_managed_hook(existing: &str) -> String {
    let existing_lines = existing.lines().collect::<Vec<_>>();
    let mut merged = Vec::new();
    let mut index = 0;
    while index < existing_lines.len() {
        if table_header(existing_lines[index]) != Some(PRE_TOOL_USE_HEADER) {
            merged.push(existing_lines[index]);
            index += 1;
            continue;
        }

        let section_start = index;
        let section_end = existing_lines
            .iter()
            .enumerate()
            .skip(index + 1)
            .find_map(|(next, line)| {
                (table_header(line) == Some(PRE_TOOL_USE_HEADER)
                    || (table_header(line).is_some()
                        && table_header(line) != Some(PRE_TOOL_USE_HOOK_HEADER)))
                .then_some(next)
            })
            .unwrap_or(existing_lines.len());
        let has_managed_hook = existing_lines[section_start..section_end]
            .iter()
            .any(|line| line.trim() == RETIRED_HOOK_COMMAND);
        if !has_managed_hook {
            merged.extend_from_slice(&existing_lines[section_start..section_end]);
            index = section_end;
            continue;
        }

        let first_hook = (section_start + 1..section_end).find(|position| {
            table_header(existing_lines[*position]) == Some(PRE_TOOL_USE_HOOK_HEADER)
        });
        let Some(first_hook) = first_hook else {
            index = section_end;
            continue;
        };
        let mut retained_hooks = Vec::new();
        let mut hook_start = first_hook;
        while hook_start < section_end {
            let hook_end = (hook_start + 1..section_end)
                .find(|position| {
                    table_header(existing_lines[*position]) == Some(PRE_TOOL_USE_HOOK_HEADER)
                })
                .unwrap_or(section_end);
            if !existing_lines[hook_start..hook_end]
                .iter()
                .any(|line| line.trim() == RETIRED_HOOK_COMMAND)
            {
                retained_hooks.extend_from_slice(&existing_lines[hook_start..hook_end]);
            }
            hook_start = hook_end;
        }
        if !retained_hooks.is_empty() {
            merged.extend_from_slice(&existing_lines[section_start..first_hook]);
            merged.extend(retained_hooks);
        }
        index = section_end;
    }
    format!("{}\n", merged.join("\n").trim_start_matches('\n'))
}

fn merge_managed_config(template: &str, existing: &str) -> String {
    let managed = managed_assignments(template, None, MANAGED_CONFIG_KEYS).join("\n");
    let managed_agents = managed_assignments(template, Some("[agents]"), MANAGED_AGENT_KEYS);

    let mut in_top_level = true;
    let mut in_workspace_sandbox = false;
    let mut in_agents = false;
    let mut in_legacy_profile_table = false;
    let mut workspace_sandbox_found = false;
    let mut agents_found = false;
    let mut preserved_lines = Vec::new();
    for line in existing.lines() {
        let trimmed = line.trim_start();
        let header = table_header(line);
        if let Some(header) = header {
            in_top_level = false;
            in_legacy_profile_table = header == "[profiles]" || header.starts_with("[profiles.");
            in_workspace_sandbox = header == "[sandbox_workspace_write]";
            in_agents = header == "[agents]";
            if in_workspace_sandbox {
                workspace_sandbox_found = true;
            }
            if in_agents {
                agents_found = true;
            }
        }
        if in_legacy_profile_table {
            continue;
        }
        if in_top_level && is_assignment_for(line, &["profile"]) {
            continue;
        }
        if in_top_level && is_assignment_for(line, MANAGED_CONFIG_KEYS) {
            continue;
        }
        if in_workspace_sandbox
            && trimmed
                .strip_prefix("network_access")
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        {
            continue;
        }
        if in_agents && is_assignment_for(line, MANAGED_AGENT_KEYS) {
            continue;
        }
        preserved_lines.push(line);
        if in_workspace_sandbox && header == Some("[sandbox_workspace_write]") {
            preserved_lines.push("network_access = true");
        }
        if in_agents && header == Some("[agents]") {
            preserved_lines.extend(managed_agents.iter().map(String::as_str));
        }
    }
    if !workspace_sandbox_found {
        preserved_lines.extend(["", "[sandbox_workspace_write]", "network_access = true"]);
    }
    if !agents_found && !managed_agents.is_empty() {
        preserved_lines.push("");
        preserved_lines.push("[agents]");
        preserved_lines.extend(managed_agents.iter().map(String::as_str));
    }
    let preserved = preserved_lines
        .join("\n")
        .trim_start_matches('\n')
        .to_string();

    let merged = if preserved.is_empty() {
        format!("{managed}\n")
    } else {
        format!("{managed}\n\n{preserved}\n")
    };
    sync_managed_hook(template, &merged)
}

fn merge_managed_config_with_root(
    template: &str,
    existing: &str,
    managed_root: &Path,
) -> Result<String> {
    let merged = merge_managed_config(template, existing);
    ensure_managed_writable_root(&merged, managed_root)
}

fn sync_managed_hook(template: &str, existing: &str) -> String {
    let cleaned = remove_retired_managed_hook(existing);
    if !template.contains(MANAGED_HOOK_COMMAND)
        || cleaned
            .lines()
            .any(|line| line.trim() == MANAGED_HOOK_COMMAND)
    {
        return cleaned;
    }
    format!("{}\n\n{MANAGED_HOOK_CONFIG}\n", cleaned.trim_end())
}

fn ensure_managed_writable_root(contents: &str, managed_root: &Path) -> Result<String> {
    let root = managed_root.to_str().with_context(|| {
        format!(
            "Managed worktree root is not valid UTF-8: {}",
            managed_root.display()
        )
    })?;
    if !managed_root.is_absolute() {
        anyhow::bail!("Managed worktree root must be absolute: {root}");
    }
    let mut document = contents
        .parse::<DocumentMut>()
        .context("Codex config must be valid TOML before adding writable_roots")?;
    let sandbox = document
        .entry("sandbox_workspace_write")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .context("sandbox_workspace_write must be a TOML table")?;
    let roots = sandbox
        .entry("writable_roots")
        .or_insert_with(|| value(Array::new()))
        .as_array_mut()
        .context("writable_roots must be a TOML string array")?;
    if roots.iter().any(|item| item.as_str().is_none()) {
        anyhow::bail!("writable_roots must contain only TOML strings");
    }
    if !roots.iter().any(|item| item.as_str() == Some(root)) {
        roots.push(root);
    }
    let mut rendered = document.to_string();
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

fn backup_legacy_config(codex_dir: &Path) -> Result<()> {
    let config_path = codex_dir.join("config.toml");
    if !config_path.exists() {
        return Ok(());
    }

    let contents = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    if !contains_legacy_profile_config(&contents) {
        return Ok(());
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before UNIX_EPOCH")?
        .as_nanos();
    let backup_path = codex_dir.join(format!("config.toml.bak.legacy.{timestamp}"));

    copy_file_exclusive(&config_path, &backup_path)?;
    println!(
        "\nLegacy Codex profile config detected. Backed up {} to {}; local settings will be preserved.",
        config_path.display(),
        backup_path.display()
    );

    Ok(())
}

fn write_file_temp(path: &Path, contents: &str, permissions: fs::Permissions) -> Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before UNIX_EPOCH")?
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("managed-file");
    let temp_path = path.with_file_name(format!(
        ".{file_name}.tmp.automation.{}.{timestamp}",
        std::process::id()
    ));
    let mut temp_created = false;
    let write_result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut temp = options
            .open(&temp_path)
            .with_context(|| format!("Failed to create temporary {}", temp_path.display()))?;
        temp_created = true;
        temp.write_all(contents.as_bytes())
            .with_context(|| format!("Failed to write temporary {}", temp_path.display()))?;
        fs::set_permissions(&temp_path, permissions)
            .with_context(|| format!("Failed to set permissions on {}", temp_path.display()))?;
        temp.sync_all()
            .with_context(|| format!("Failed to sync temporary {}", temp_path.display()))?;
        Ok(())
    })();
    if let Err(error) = write_result {
        if temp_created {
            let _ = fs::remove_file(&temp_path);
        }
        return Err(error);
    }
    Ok(temp_path)
}

fn replace_regular_file(path: &Path, contents: &str, permissions: fs::Permissions) -> Result<()> {
    let temp_path = write_file_temp(path, contents, permissions)?;
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error).with_context(|| format!("Failed to update {}", path.display()));
    }
    Ok(())
}

fn managed_hook_state_path(hook_path: &Path) -> PathBuf {
    let file_name = hook_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("block_git_write.py");
    hook_path.with_file_name(format!("{file_name}{MANAGED_HOOK_STATE_SUFFIX}"))
}

fn sha256(contents: &str) -> String {
    format!("{:x}", Sha256::digest(contents.as_bytes()))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn regular_file_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => anyhow::bail!(
            "Managed hook state {} must be a regular file",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("Failed to inspect {}", path.display())),
    }
}

fn reject_symlink_directory_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!(
                    "Symlink directory component is not allowed: {}",
                    current.display()
                )
            }
            Ok(metadata) if !metadata.is_dir() => {
                anyhow::bail!(
                    "Managed path component is not a directory: {}",
                    current.display()
                )
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to inspect {}", current.display()));
            }
        }
    }
    Ok(())
}

fn installed_permissions(source: &fs::Permissions, mode: ManagedInstallMode) -> fs::Permissions {
    #[cfg(unix)]
    if mode == ManagedInstallMode::OwnerExecutable {
        return fs::Permissions::from_mode(0o700);
    }
    source.clone()
}

fn repair_managed_permissions(
    destination: &Path,
    expected: &fs::Permissions,
    mode: ManagedInstallMode,
) -> Result<bool> {
    if mode == ManagedInstallMode::PreserveSource {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        let current = fs::metadata(destination)
            .with_context(|| {
                format!(
                    "Failed to inspect permissions for {}",
                    destination.display()
                )
            })?
            .permissions();
        if current.mode() & 0o7777 != expected.mode() & 0o7777 {
            fs::set_permissions(destination, expected.clone()).with_context(|| {
                format!("Failed to repair permissions on {}", destination.display())
            })?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_managed_hook(
    source: &Path,
    destination: &Path,
    install_mode: ManagedInstallMode,
) -> Result<()> {
    if let Some(parent) = destination.parent() {
        reject_symlink_directory_components(parent)?;
        if !parent.exists() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create managed hook directory {}",
                    parent.display()
                )
            })?;
        }
    }
    let source_contents = fs::read_to_string(source)
        .with_context(|| format!("Failed to read managed hook source {}", source.display()))?;
    let source_permissions = fs::metadata(source)
        .with_context(|| format!("Failed to inspect managed hook source {}", source.display()))?
        .permissions();
    let destination_permissions = installed_permissions(&source_permissions, install_mode);
    let source_hash = sha256(&source_contents);
    let state_path = managed_hook_state_path(destination);
    let state_exists = regular_file_exists(&state_path)?;
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect {}", destination.display()));
        }
    };

    if metadata.is_none() {
        if state_exists {
            anyhow::bail!(
                "Managed hook state exists without hook {}; refusing to overwrite",
                destination.display()
            );
        }
        copy_file_exclusive_with_permissions(source, destination, destination_permissions.clone())?;
        if let Err(error) = write_file_exclusive(&state_path, &source_hash, source_permissions) {
            let _ = fs::remove_file(destination);
            return Err(error);
        }
        println!("- Installed local managed hook: {}", destination.display());
        return Ok(());
    }

    if metadata
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        verify_managed_symlink(source, destination)?;
        if state_exists {
            anyhow::bail!(
                "Managed hook state already exists for symlink {}; refusing to migrate",
                destination.display()
            );
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("System clock is before UNIX_EPOCH")?
            .as_nanos();
        let backup_path = destination.with_file_name(format!(
            "{}.bak.automation-link.{timestamp}",
            destination
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("block_git_write.py")
        ));
        copy_symlink_exclusive(destination, &backup_path)?;
        replace_regular_file(
            destination,
            &source_contents,
            destination_permissions.clone(),
        )?;
        write_file_exclusive(&state_path, &source_hash, source_permissions)?;
        println!(
            "- Migrated managed hook to local copy (symlink backup: {}).",
            backup_path.display()
        );
        return Ok(());
    }

    let existing = fs::read_to_string(destination)
        .with_context(|| format!("Failed to read managed hook {}", destination.display()))?;
    let existing_hash = sha256(&existing);
    let recorded = if state_exists {
        Some(
            fs::read_to_string(&state_path)
                .with_context(|| {
                    format!("Failed to read managed hook state {}", state_path.display())
                })?
                .trim()
                .to_owned(),
        )
    } else {
        None
    };
    let state_matches = recorded.as_deref().is_some_and(valid_sha256)
        && recorded.as_deref() == Some(existing_hash.as_str());
    if install_mode == ManagedInstallMode::OwnerExecutable && recorded.is_none() {
        anyhow::bail!(
            "Managed state is missing for {}; refusing to adopt an unmanaged file",
            destination.display()
        );
    }
    if existing_hash != source_hash && !state_matches {
        anyhow::bail!(
            "Managed hook {} changed locally; refusing to overwrite",
            destination.display()
        );
    }
    if existing_hash == source_hash && recorded.as_deref() == Some(source_hash.as_str()) {
        if repair_managed_permissions(destination, &destination_permissions, install_mode)? {
            println!(
                "- Repaired local managed file permissions: {}.",
                destination.display()
            );
        } else {
            println!("- Local managed hook is up to date.");
        }
        return Ok(());
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before UNIX_EPOCH")?
        .as_nanos();
    let backup_path = destination.with_file_name(format!(
        "{}.bak.automation.{timestamp}",
        destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("block_git_write.py")
    ));
    copy_file_exclusive(destination, &backup_path)?;
    replace_regular_file(destination, &source_contents, destination_permissions)?;
    replace_regular_file(&state_path, &source_hash, source_permissions)?;
    println!(
        "- Updated local managed hook (backup: {}).",
        backup_path.display()
    );
    Ok(())
}

fn ensure_config_unchanged(
    config_path: &Path,
    expected: &str,
    expected_symlink: bool,
) -> Result<()> {
    let metadata = fs::symlink_metadata(config_path)
        .with_context(|| format!("Failed to re-inspect {}", config_path.display()))?;
    if metadata.file_type().is_symlink() != expected_symlink {
        anyhow::bail!(
            "{} changed type while settings were being migrated",
            config_path.display()
        );
    }
    let current = fs::read_to_string(config_path)
        .with_context(|| format!("Failed to re-read {}", config_path.display()))?;
    if current != expected {
        anyhow::bail!(
            "{} changed while settings were being migrated; retry setup",
            config_path.display()
        );
    }
    Ok(())
}

fn migrate_managed_config_from_template(codex_dir: &Path, template_path: &Path) -> Result<()> {
    let managed_root = canonical_or_absolute(&codex_dir.join("worktrees"))?;
    migrate_managed_config_from_template_with_root(codex_dir, template_path, &managed_root)
}

fn migrate_managed_config_from_template_with_root(
    codex_dir: &Path,
    template_path: &Path,
    managed_root: &Path,
) -> Result<()> {
    let config_path = codex_dir.join("config.toml");
    let metadata = match fs::symlink_metadata(&config_path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect {}", config_path.display()));
        }
    };
    let is_symlink = metadata
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink());

    if metadata.is_none() {
        let template = fs::read_to_string(template_path)
            .with_context(|| format!("Failed to read {}", template_path.display()))?;
        let default_root = home::home_dir().map(|home| home.join(".codex/worktrees"));
        let installed = if default_root
            .as_deref()
            .and_then(|root| canonical_or_absolute(root).ok())
            .is_some_and(|root| root == managed_root)
        {
            template.clone()
        } else {
            ensure_managed_writable_root(&template, managed_root)?
        };
        let permissions = fs::metadata(template_path)
            .with_context(|| {
                format!(
                    "Failed to inspect permissions for {}",
                    template_path.display()
                )
            })?
            .permissions();
        if installed == template {
            copy_file_exclusive(template_path, &config_path)?;
        } else {
            write_file_exclusive(&config_path, &installed, permissions)?;
        }
        println!("- Installed base config: {}", config_path.display());
        return Ok(());
    }

    let existing = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    let template = fs::read_to_string(template_path)
        .with_context(|| format!("Failed to read {}", template_path.display()))?;
    let migrated = merge_managed_config_with_root(&template, &existing, managed_root)?;
    if migrated == existing && !is_symlink {
        println!("- Shared Codex settings are up to date.");
        return Ok(());
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before UNIX_EPOCH")?
        .as_nanos();
    let backup_path = codex_dir.join(format!("config.toml.bak.automation.{timestamp}"));
    copy_file_exclusive(&config_path, &backup_path)?;
    let permissions = fs::metadata(&config_path)
        .with_context(|| {
            format!(
                "Failed to inspect permissions for {}",
                config_path.display()
            )
        })?
        .permissions();
    let temp_path = write_file_temp(&config_path, &migrated, permissions)?;
    if let Err(error) = ensure_config_unchanged(&config_path, &existing, is_symlink) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    if is_symlink {
        let symlink_backup_path =
            codex_dir.join(format!("config.toml.bak.automation-link.{timestamp}"));
        if let Err(error) = copy_symlink_exclusive(&config_path, &symlink_backup_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
        if let Err(write_error) = fs::rename(&temp_path, &config_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(write_error).context(format!(
                "Failed to atomically replace symlink {}; the original remains in place",
                config_path.display()
            ));
        }
        println!(
            "- Updated shared Codex settings (contents backup: {}, symlink backup: {}).",
            backup_path.display(),
            symlink_backup_path.display()
        );
        return Ok(());
    }
    if let Err(error) = fs::rename(&temp_path, &config_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error).with_context(|| format!("Failed to update {}", config_path.display()));
    }
    println!(
        "- Updated shared Codex settings (backup: {}).",
        backup_path.display()
    );
    Ok(())
}

fn migrate_managed_config(codex_dir: &Path) -> Result<()> {
    let dotfiles_path = std::env::current_dir().context("Failed to get current directory")?;
    let template_path = dotfiles_path.join(CODEX_CONFIG_TEMPLATE);
    migrate_managed_config_from_template(codex_dir, &template_path)
}

fn archive_retired_profiles(codex_dir: &Path) -> Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before UNIX_EPOCH")?
        .as_secs();
    for name in ["teacher.config.toml", "autonomous.config.toml"] {
        let profile_path = codex_dir.join(name);
        if !profile_path.exists() {
            continue;
        }
        let backup_path = codex_dir.join(format!("{name}.bak.retired.{timestamp}"));
        fs::rename(&profile_path, &backup_path).with_context(|| {
            format!(
                "Failed to archive retired profile {} to {}",
                profile_path.display(),
                backup_path.display()
            )
        })?;
        println!(
            "- Archived retired profile {} to {}.",
            profile_path.display(),
            backup_path.display()
        );
    }
    Ok(())
}

fn validate_absolute_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("CODEX_HOME must be an absolute path: {}", path.display());
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        anyhow::bail!(
            "CODEX_HOME must not contain parent traversal: {}",
            path.display()
        );
    }

    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!(
                    "symlink path component is not allowed: {}",
                    current.display()
                );
            }
            Ok(metadata) if !metadata.file_type().is_dir() && current != path => {
                anyhow::bail!("path component is not a directory: {}", current.display());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to inspect path {}", current.display()));
            }
        }
    }
    Ok(())
}

fn resolve_codex_home(home: &Path) -> Result<PathBuf> {
    let configured = std::env::var_os("CODEX_HOME");
    let candidate = configured
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    validate_absolute_path(&candidate)?;
    if let Ok(metadata) = fs::symlink_metadata(&candidate) {
        if metadata.file_type().is_symlink() {
            anyhow::bail!("CODEX_HOME must not be a symlink: {}", candidate.display());
        }
        if !metadata.file_type().is_dir() {
            anyhow::bail!("CODEX_HOME must be a directory: {}", candidate.display());
        }
    }
    Ok(candidate)
}

fn validate_setup_home(configured: PathBuf, account: PathBuf) -> Result<PathBuf> {
    validate_absolute_path(&configured)?;
    validate_absolute_path(&account)?;
    if configured != account {
        anyhow::bail!(
            "HOME must match the current account home: configured={}, account={}",
            configured.display(),
            account.display()
        );
    }
    Ok(account)
}

#[cfg(unix)]
fn account_home_dir() -> Result<PathBuf> {
    const DEFAULT_BUFFER: usize = 16 * 1024;
    const MAX_BUFFER: usize = 1024 * 1024;
    // SAFETY: getuid and sysconf do not dereference pointers or retain state supplied by Rust.
    let uid = unsafe { libc::getuid() };
    let suggested = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut size = if suggested > 0 {
        usize::try_from(suggested).unwrap_or(DEFAULT_BUFFER)
    } else {
        DEFAULT_BUFFER
    }
    .clamp(DEFAULT_BUFFER, MAX_BUFFER);
    loop {
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0_u8; size];
        // SAFETY: record, buffer, and result are valid writable storage for the duration of
        // getpwuid_r. The returned pw_dir points into buffer and is copied before it is dropped.
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && size < MAX_BUFFER {
            size = (size * 2).min(MAX_BUFFER);
            continue;
        }
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status))
                .context("Failed to resolve current account home");
        }
        if result.is_null() {
            anyhow::bail!("Current account is missing from the password database");
        }
        // SAFETY: a successful getpwuid_r initialized record and returned a non-null result.
        let record = unsafe { record.assume_init() };
        if record.pw_dir.is_null() {
            anyhow::bail!("Current account home is missing from the password database");
        }
        // SAFETY: POSIX passwd strings are NUL-terminated and remain valid while buffer lives.
        let bytes = unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes();
        if bytes.is_empty() {
            anyhow::bail!("Current account home is empty");
        }
        return Ok(PathBuf::from(OsStr::from_bytes(bytes)));
    }
}

#[cfg(unix)]
fn setup_home_dir() -> Result<PathBuf> {
    let configured = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .context("HOME must be set to the current account home")?;
    validate_setup_home(configured, account_home_dir()?)
}

#[cfg(not(unix))]
fn setup_home_dir() -> Result<PathBuf> {
    let home = home::home_dir().context("Cannot find home directory")?;
    validate_absolute_path(&home)?;
    Ok(home)
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    validate_absolute_path(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!(
                "Managed directory must not be a symlink: {}",
                path.display()
            );
        }
        Ok(metadata) if !metadata.file_type().is_dir() => {
            anyhow::bail!("Managed path must be a directory: {}", path.display());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .with_context(|| format!("Failed to create directory {}", path.display()))?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect directory {}", path.display()));
        }
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Failed to inspect directory {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        anyhow::bail!(
            "Managed path must be a regular directory: {}",
            path.display()
        );
    }
    #[cfg(unix)]
    fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .with_context(|| format!("Failed to set private permissions on {}", path.display()))?;
    Ok(())
}

fn canonical_or_absolute(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        fs::canonicalize(path)
            .with_context(|| format!("Failed to resolve managed path {}", path.display()))
    } else {
        Ok(path.to_path_buf())
    }
}

pub fn setup() -> Result<()> {
    println!("🧠 Setting up Codex CLI...\n");
    let home = setup_home_dir()?;

    if !is_codex_installed() {
        println!("Codex CLI is not found.");
        codex_install()?;
    } else {
        println!("Codex CLI is already installed.");
    }

    codex_check()?;

    let helper_directory = home.join(".local/bin");
    let helper_on_path = std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|path| path == helper_directory));
    if !helper_on_path {
        anyhow::bail!(
            "{} must be present in PATH before installing Codex helpers",
            helper_directory.display()
        );
    }
    let codex_dir = resolve_codex_home(&home)?;
    if !codex_dir.exists() {
        println!("\nCreating CODEX_HOME directory: {}", codex_dir.display());
    }
    ensure_private_directory(&codex_dir)?;
    let managed_worktree_root = codex_dir.join("worktrees");
    ensure_private_directory(&managed_worktree_root)?;

    println!("\nLinking shared configuration files...");
    for (source, dest) in CODEX_FILES {
        ensure_shared_symlink(source, dest)?;
    }
    let dotfiles_path = std::env::current_dir().context("Failed to get current directory")?;
    ensure_managed_hook(
        &dotfiles_path.join(MANAGED_HOOK_SOURCE),
        &home.join(MANAGED_HOOK_DESTINATION),
        ManagedInstallMode::PreserveSource,
    )?;
    for (source, destination, install_mode) in MANAGED_HELPERS {
        ensure_managed_hook(
            &dotfiles_path.join(source),
            &home.join(destination),
            *install_mode,
        )?;
    }
    println!("\nLinking shared skill directories...");
    for (source, dest) in CODEX_DIRS {
        ensure_shared_symlink(source, dest)?;
    }

    backup_legacy_config(&codex_dir)?;
    archive_retired_profiles(&codex_dir)?;

    println!("\nMigrating shared Codex settings...");
    migrate_managed_config(&codex_dir)?;

    println!("\n✅ Codex CLI setup completed!");
    println!("\n💡 Next steps:");
    println!("   1. Run 'codex login' if authentication is not configured");
    println!("   2. Run 'codex' (workspace-write + auto-review is the default)");
    println!("   3. Restart Codex after installation so writable roots are reloaded");

    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::account_home_dir;
    use super::{
        MANAGED_HELPERS, MANAGED_HOOK_COMMAND, ManagedInstallMode, RETIRED_HOOK_COMMAND,
        contains_legacy_profile_config, copy_file_exclusive, ensure_config_unchanged,
        ensure_managed_hook, ensure_managed_writable_root, ensure_private_directory,
        managed_hook_state_path, merge_managed_config, merge_managed_config_with_root,
        migrate_managed_config_from_template, sha256, validate_setup_home, verify_managed_symlink,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDirectory(PathBuf);

    #[test]
    fn managed_helpers_include_worktree_and_delivery() {
        assert_eq!(
            MANAGED_HELPERS,
            &[
                (
                    ".codex/helpers/codex-worktree",
                    ".local/bin/codex-worktree",
                    ManagedInstallMode::PreserveSource,
                ),
                (
                    ".codex/helpers/codex-delivery",
                    ".local/bin/codex-delivery",
                    ManagedInstallMode::OwnerExecutable,
                ),
            ]
        );
    }

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is after UNIX_EPOCH")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "dotfiles-codex-{label}-{}-{timestamp}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create temporary test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn managed_keys_are_replaced_and_local_tables_are_preserved() {
        let template = r#"
model = "gpt-5.6-terra"
approval_policy = "on-request"
approvals_reviewer = "auto_review"
sandbox_mode = "workspace-write"
commit_attribution = ""

[sandbox_workspace_write]
network_access = true

[agents]
enabled = true
max_concurrent_threads_per_session = 3
default_subagent_model = "gpt-5.6-terra"
default_subagent_reasoning_effort = "medium"
"#;
        let existing = r#"model = "old"
sandbox_mode = "read-only"

[sandbox_workspace_write]
network_access = false
exclude_tmpdir_env_var = true

[projects."/work"]
trust_level = "trusted"

[hooks.state]
trusted_hash = "local-value"

[agents]
enabled = false
max_concurrent_threads_per_session = 12
default_subagent_model = "old"
custom_setting = "preserved"

[agents.local_reviewer]
description = "local"
"#;

        let actual = merge_managed_config(template, existing);
        assert!(actual.contains("model = \"gpt-5.6-terra\""));
        assert!(actual.contains("sandbox_mode = \"workspace-write\""));
        assert!(actual.contains("approvals_reviewer = \"auto_review\""));
        assert!(actual.contains("commit_attribution = \"\""));
        assert!(actual.contains("[sandbox_workspace_write]\nnetwork_access = true"));
        assert!(actual.contains("exclude_tmpdir_env_var = true"));
        assert!(!actual.contains("network_access = false"));
        assert!(!actual.contains("model = \"old\""));
        assert!(actual.contains("[projects.\"/work\"]"));
        assert!(actual.contains("trusted_hash = \"local-value\""));
        assert!(actual.contains("[agents]\nenabled = true"));
        assert!(actual.contains("max_concurrent_threads_per_session = 3"));
        assert!(actual.contains("default_subagent_model = \"gpt-5.6-terra\""));
        assert!(actual.contains("default_subagent_reasoning_effort = \"medium\""));
        assert!(actual.contains("custom_setting = \"preserved\""));
        assert!(actual.contains("[agents.local_reviewer]\ndescription = \"local\""));
        assert!(!actual.contains("max_concurrent_threads_per_session = 12"));
        assert!(!actual.contains("default_subagent_model = \"old\""));
        assert_eq!(merge_managed_config(template, &actual), actual);
    }

    #[test]
    fn exclusive_backup_does_not_overwrite_an_existing_path() {
        let directory = TestDirectory::new("exclusive-backup");
        let source = directory.path().join("source.toml");
        let destination = directory.path().join("backup.toml");
        fs::write(&source, "new").expect("write backup source");
        fs::write(&destination, "existing").expect("write existing backup");

        assert!(copy_file_exclusive(&source, &destination).is_err());
        assert_eq!(
            fs::read_to_string(&destination).expect("read existing backup"),
            "existing"
        );
    }

    #[test]
    fn concurrent_config_change_is_detected_before_replacement() {
        let directory = TestDirectory::new("concurrent-config");
        let config = directory.path().join("config.toml");
        fs::write(&config, "model = \"old\"\n").expect("write initial config");
        assert!(ensure_config_unchanged(&config, "model = \"old\"\n", false).is_ok());

        fs::write(&config, "model = \"local-change\"\n").expect("write concurrent change");
        assert!(ensure_config_unchanged(&config, "model = \"old\"\n", false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn managed_paths_must_be_symlinks_to_the_expected_source() {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new("managed-symlink");
        let expected = directory.path().join("expected");
        let other = directory.path().join("other");
        let correct_link = directory.path().join("correct-link");
        let wrong_link = directory.path().join("wrong-link");
        let regular = directory.path().join("regular");
        fs::write(&expected, "expected").expect("write expected source");
        fs::write(&other, "other").expect("write other source");
        fs::write(&regular, "expected").expect("write regular destination");
        symlink(&expected, &correct_link).expect("create correct symlink");
        symlink(&other, &wrong_link).expect("create wrong symlink");

        assert!(verify_managed_symlink(&expected, &correct_link).is_ok());
        assert!(verify_managed_symlink(&expected, &wrong_link).is_err());
        assert!(verify_managed_symlink(&expected, &regular).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn managed_hook_migrates_expected_symlink_to_regular_copy() {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new("managed-hook-symlink");
        let source = directory.path().join("source.py");
        let destination = directory.path().join("hook.py");
        fs::write(&source, "old hook\n").expect("write source");
        symlink(&source, &destination).expect("create managed symlink");

        ensure_managed_hook(&source, &destination, ManagedInstallMode::PreserveSource)
            .expect("migrate managed hook");

        assert!(
            !fs::symlink_metadata(&destination)
                .expect("inspect destination")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(&destination).expect("read hook"),
            "old hook\n"
        );
        assert_eq!(
            fs::read_to_string(managed_hook_state_path(&destination)).expect("read state"),
            sha256("old hook\n")
        );
    }

    #[test]
    fn managed_hook_updates_only_when_state_matches_current_copy() {
        let directory = TestDirectory::new("managed-hook-update");
        let source = directory.path().join("source.py");
        let destination = directory.path().join("hook.py");
        fs::write(&source, "new hook\n").expect("write source");
        fs::write(&destination, "old hook\n").expect("write hook");
        fs::write(managed_hook_state_path(&destination), sha256("old hook\n"))
            .expect("write state");

        ensure_managed_hook(&source, &destination, ManagedInstallMode::PreserveSource)
            .expect("update managed hook");
        assert_eq!(
            fs::read_to_string(&destination).expect("read hook"),
            "new hook\n"
        );

        fs::write(&destination, "local change\n").expect("change hook");
        assert!(
            ensure_managed_hook(&source, &destination, ManagedInstallMode::PreserveSource,)
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn delivery_helper_is_installed_private_and_repairs_managed_mode() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TestDirectory::new("delivery-helper-mode");
        let source = directory.path().join("codex-delivery");
        let destination = directory.path().join("bin").join("codex-delivery");
        fs::write(&source, "#!/bin/sh\n").expect("write delivery helper");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).expect("set source mode");

        ensure_managed_hook(&source, &destination, ManagedInstallMode::OwnerExecutable)
            .expect("install delivery helper");
        assert_eq!(
            fs::metadata(&destination)
                .expect("inspect installed helper")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::read_to_string(managed_hook_state_path(&destination)).expect("read state"),
            sha256("#!/bin/sh\n")
        );

        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))
            .expect("introduce managed mode drift");
        ensure_managed_hook(&source, &destination, ManagedInstallMode::OwnerExecutable)
            .expect("repair managed delivery helper mode");
        assert_eq!(
            fs::metadata(&destination)
                .expect("inspect repaired helper")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[cfg(unix)]
    #[test]
    fn delivery_helper_does_not_chmod_a_local_modification() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TestDirectory::new("delivery-helper-local");
        let source = directory.path().join("codex-delivery");
        let destination = directory.path().join("codex-delivery-installed");
        fs::write(&source, "reviewed\n").expect("write delivery helper");
        ensure_managed_hook(&source, &destination, ManagedInstallMode::OwnerExecutable)
            .expect("install delivery helper");
        fs::write(&destination, "local change\n").expect("change installed helper");
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))
            .expect("set local mode");

        assert!(
            ensure_managed_hook(&source, &destination, ManagedInstallMode::OwnerExecutable)
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(&destination).expect("read local helper"),
            "local change\n"
        );
        assert_eq!(
            fs::metadata(&destination)
                .expect("inspect local helper")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[test]
    fn delivery_helper_does_not_adopt_an_unmanaged_file() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TestDirectory::new("delivery-helper-unmanaged");
        let source = directory.path().join("codex-delivery");
        let destination = directory.path().join("codex-delivery-installed");
        fs::write(&source, "same contents\n").expect("write delivery helper");
        fs::write(&destination, "same contents\n").expect("write unmanaged helper");
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))
            .expect("set unmanaged mode");

        assert!(
            ensure_managed_hook(&source, &destination, ManagedInstallMode::OwnerExecutable)
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(&destination).expect("read unmanaged helper"),
            "same contents\n"
        );
        assert_eq!(
            fs::metadata(&destination)
                .expect("inspect unmanaged helper")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert!(!managed_hook_state_path(&destination).exists());
    }

    #[test]
    fn setup_home_must_match_the_account_home() {
        let account = PathBuf::from("/home/account");
        assert_eq!(
            validate_setup_home(account.clone(), account.clone()).expect("matching home"),
            account
        );
        assert!(
            validate_setup_home(
                PathBuf::from("/home/configured"),
                PathBuf::from("/home/account"),
            )
            .is_err()
        );
        assert!(
            validate_setup_home(PathBuf::from("relative"), PathBuf::from("/home/account")).is_err()
        );
        assert!(validate_setup_home(PathBuf::new(), PathBuf::from("/home/account")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn current_account_home_is_resolved_from_the_password_database() {
        let account_home = account_home_dir().expect("resolve current account home");
        assert!(account_home.is_absolute());
        assert!(!account_home.as_os_str().is_empty());
    }

    #[test]
    fn managed_hook_repairs_missing_state_when_hook_matches_source() {
        let directory = TestDirectory::new("managed-hook-repair");
        let source = directory.path().join("source.py");
        let destination = directory.path().join("hook.py");
        fs::write(&source, "hook\n").expect("write source");
        fs::write(&destination, "hook\n").expect("write hook");

        ensure_managed_hook(&source, &destination, ManagedInstallMode::PreserveSource)
            .expect("repair missing state");

        assert_eq!(
            fs::read_to_string(managed_hook_state_path(&destination)).expect("read state"),
            sha256("hook\n")
        );
    }

    #[test]
    fn managed_hook_creates_missing_parent_directory() {
        let directory = TestDirectory::new("managed-hook-parent");
        let source = directory.path().join("source.py");
        let destination = directory.path().join("hooks").join("hook.py");
        fs::write(&source, "hook\n").expect("write source");

        ensure_managed_hook(&source, &destination, ManagedInstallMode::PreserveSource)
            .expect("install hook in new directory");

        assert_eq!(
            fs::read_to_string(&destination).expect("read hook"),
            "hook\n"
        );
        assert_eq!(
            fs::read_to_string(managed_hook_state_path(&destination)).expect("read state"),
            sha256("hook\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn managed_hook_rejects_symlinked_parent_directory() {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new("managed-hook-parent-symlink");
        let source = directory.path().join("source.py");
        let real_parent = directory.path().join("real-hooks");
        let linked_parent = directory.path().join("linked-hooks");
        fs::write(&source, "hook\n").expect("write source");
        fs::create_dir(&real_parent).expect("create real parent");
        symlink(&real_parent, &linked_parent).expect("create parent symlink");

        assert!(
            ensure_managed_hook(
                &source,
                &linked_parent.join("hook.py"),
                ManagedInstallMode::PreserveSource,
            )
            .is_err()
        );
        assert!(!real_parent.join("hook.py").exists());
    }

    #[test]
    fn workspace_sandbox_table_is_added_when_missing() {
        let actual = merge_managed_config(
            "model = \"gpt-5.6-terra\"\n\n[agents]\nenabled = true\ndefault_subagent_model = \"gpt-5.6-terra\"\n",
            "model = \"old\"\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n",
        );
        assert!(actual.contains("[sandbox_workspace_write]\nnetwork_access = true"));
        assert!(
            actual.contains("[agents]\nenabled = true\ndefault_subagent_model = \"gpt-5.6-terra\"")
        );
        assert!(actual.contains("[projects.\"/work\"]"));
    }

    #[test]
    fn commented_managed_table_headers_are_reused_and_idempotent() {
        let template = r#"model = "gpt-5.6-terra"

[sandbox_workspace_write]
network_access = true

[agents]
enabled = true
max_concurrent_threads_per_session = 3
default_subagent_model = "gpt-5.6-terra"
"#;
        let existing = r#"model = "old"

[sandbox_workspace_write]   # local sandbox options
network_access = false
exclude_tmpdir_env_var = true

[agents] # local agent defaults
enabled = false
max_concurrent_threads_per_session = 12
custom_setting = "preserved"

[agents.local_reviewer] # custom agent
description = "local"
"#;

        let actual = merge_managed_config(template, existing);
        assert!(actual.contains(
            "[sandbox_workspace_write]   # local sandbox options\nnetwork_access = true"
        ));
        assert!(actual.contains("exclude_tmpdir_env_var = true"));
        assert!(!actual.contains("network_access = false"));
        assert!(actual.contains("[agents] # local agent defaults\nenabled = true"));
        assert!(actual.contains("max_concurrent_threads_per_session = 3"));
        assert!(actual.contains("custom_setting = \"preserved\""));
        assert!(actual.contains("[agents.local_reviewer] # custom agent\ndescription = \"local\""));
        assert!(!actual.contains("\n[sandbox_workspace_write]\n"));
        assert!(!actual.contains("\n[agents]\n"));
        assert_eq!(merge_managed_config(template, &actual), actual);
    }

    #[test]
    fn retired_hook_is_replaced_without_removing_local_hook_state() {
        let template = r#"model = "gpt-5.6-terra"

[[hooks.PreToolUse]]
matcher = "^Bash$"

[[hooks.PreToolUse.hooks]]
type = "command"
command = 'python3 "$HOME/.codex/hooks/block_git_write.py"'
timeout = 10
statusMessage = "Git/GitHub操作を確認中""#;
        let old = r#"model = "old"

[[hooks.PreToolUse]]
matcher = ".*"

[[hooks.PreToolUse.hooks]]
type = "command"
command = 'python3 "$HOME/.codex/hooks/prevent_irreversible_git.py"'
timeout = 1

[[hooks.PreToolUse.hooks]]
type = "command"
command = "local-check"
timeout = 30

[[hooks.PreToolUse.hooks]]
type = "command"
command = "echo prevent_irreversible_git.py"
timeout = 30

[hooks.state]
"#;
        let updated = merge_managed_config(template, old);
        assert!(updated.contains(MANAGED_HOOK_COMMAND));
        assert!(!updated.contains(RETIRED_HOOK_COMMAND));
        assert!(updated.contains("matcher = \".*\""));
        assert!(!updated.lines().any(|line| line == "timeout = 1"));
        assert!(updated.contains("command = \"local-check\"\ntimeout = 30"));
        assert!(updated.contains("command = \"echo prevent_irreversible_git.py\"\ntimeout = 30"));
        assert!(updated.contains("[hooks.state]"));
        assert_eq!(merge_managed_config(template, &updated), updated);
    }

    #[test]
    fn legacy_profile_settings_are_removed_while_local_tables_are_preserved() {
        let template = r#"model = "gpt-5.6-terra"

[agents]
enabled = true
"#;
        let existing = r#"profile = "teacher"
model = "old"

[projects."/work"]
trust_level = "trusted"

[profiles.teacher]
model = "gpt-5.6-sol"
sandbox_mode = "read-only"

[hooks.state]

[hooks.state."local"]
trusted_hash = "sha256:local"

[tui]
notifications = false

[agents.local_reviewer]
description = "local"
"#;

        let actual = merge_managed_config(template, existing);
        assert!(!actual.contains("profile = \"teacher\""));
        assert!(!actual.contains("[profiles.teacher]"));
        assert!(actual.contains("[projects.\"/work\"]\ntrust_level = \"trusted\""));
        assert!(actual.contains("[hooks.state.\"local\"]\ntrusted_hash = \"sha256:local\""));
        assert!(actual.contains("[tui]\nnotifications = false"));
        assert!(actual.contains("[agents.local_reviewer]\ndescription = \"local\""));
        assert_eq!(merge_managed_config(template, &actual), actual);
    }

    #[cfg(unix)]
    #[test]
    fn migration_replaces_config_symlink_without_writing_its_target() {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new("symlink-migration");
        let target_path = directory.path().join("local-config.toml");
        let config_path = directory.path().join("config.toml");
        let template_path = directory.path().join("template.toml");
        let original = "model = \"old\"\n\n[agents]\nenabled = false\n";
        let template = "model = \"gpt-5.6-terra\"\n\n[agents]\nenabled = true\n";
        fs::write(&target_path, original).expect("write symlink target");
        fs::write(&template_path, template).expect("write template");
        symlink(&target_path, &config_path).expect("create config symlink");

        migrate_managed_config_from_template(directory.path(), &template_path)
            .expect("migrate symlinked config");

        assert_eq!(
            fs::read_to_string(&target_path).expect("read symlink target"),
            original
        );
        let config_metadata = fs::symlink_metadata(&config_path).expect("inspect config");
        assert!(config_metadata.file_type().is_file());
        assert!(!config_metadata.file_type().is_symlink());
        assert!(
            fs::read_to_string(&config_path)
                .expect("read migrated config")
                .contains("model = \"gpt-5.6-terra\"")
        );

        let entries = fs::read_dir(directory.path())
            .expect("read backup directory")
            .collect::<std::result::Result<Vec<_>, _>>()
            .expect("read backup entries");
        let contents_backup = entries
            .iter()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.bak.automation.")
            })
            .expect("find contents backup");
        assert_eq!(
            fs::read_to_string(contents_backup.path()).expect("read contents backup"),
            original
        );
        let symlink_backup = entries
            .iter()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.bak.automation-link.")
            })
            .expect("find symlink backup");
        assert!(
            fs::symlink_metadata(symlink_backup.path())
                .expect("inspect symlink backup")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(symlink_backup.path()).expect("read symlink backup"),
            target_path
        );
    }

    #[cfg(unix)]
    #[test]
    fn migration_atomically_updates_regular_config_and_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TestDirectory::new("regular-migration");
        let config_path = directory.path().join("config.toml");
        let template_path = directory.path().join("template.toml");
        let original = "model = \"old\"\n\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n";
        let template = "model = \"gpt-5.6-terra\"\n";
        fs::write(&config_path, original).expect("write regular config");
        fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600))
            .expect("set config permissions");
        fs::write(&template_path, template).expect("write template");

        migrate_managed_config_from_template(directory.path(), &template_path)
            .expect("migrate regular config");

        let migrated = fs::read_to_string(&config_path).expect("read migrated config");
        assert!(migrated.contains("model = \"gpt-5.6-terra\""));
        assert!(migrated.contains("[projects.\"/work\"]\ntrust_level = \"trusted\""));
        assert_eq!(
            fs::metadata(&config_path)
                .expect("inspect migrated config")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let entries = fs::read_dir(directory.path())
            .expect("read migration directory")
            .collect::<std::result::Result<Vec<_>, _>>()
            .expect("read migration entries");
        assert!(!entries.iter().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".tmp.automation.")
        }));
        let backup = entries
            .iter()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.bak.automation.")
            })
            .expect("find regular config backup");
        assert_eq!(
            fs::read_to_string(backup.path()).expect("read regular config backup"),
            original
        );
    }

    #[test]
    fn writable_roots_preserve_local_values_and_are_idempotent() {
        let directory = TestDirectory::new("writable-roots");
        let managed_root = directory.path().join("codex").join("worktrees");
        fs::create_dir_all(&managed_root).expect("create managed root");
        let template = r#"model = "gpt-5.6-terra"

[sandbox_workspace_write]
network_access = true
writable_roots = ["~/.codex/worktrees"]
"#;
        let existing = r#"model = "old"

[sandbox_workspace_write] # local options
network_access = false
writable_roots = ["./relative", "/srv/other"] # keep these roots
exclude_tmpdir_env_var = true
"#;

        let merged = merge_managed_config_with_root(template, existing, &managed_root)
            .expect("merge writable roots");
        assert!(merged.contains("\"./relative\""));
        assert!(merged.contains("\"/srv/other\""));
        assert!(merged.contains(&format!("\"{}\"", managed_root.display())));
        assert!(merged.contains("# keep these roots"));
        assert_eq!(
            ensure_managed_writable_root(&merged, &managed_root).expect("repeat merge"),
            merged
        );
    }

    #[test]
    fn writable_roots_accept_multiline_toml_and_preserve_comments_and_escapes() {
        let directory = TestDirectory::new("multiline-writable-roots");
        let managed_root = directory.path().join("codex").join("worktrees");
        fs::create_dir_all(&managed_root).expect("create managed root");
        let existing = r#"[sandbox_workspace_write] # local options
writable_roots = [
  "/srv/other", # keep this root
  "C:\\Users\\example",
]
exclude_tmpdir_env_var = true
"#;

        let merged = ensure_managed_writable_root(existing, &managed_root)
            .expect("merge multiline writable roots");
        assert!(merged.contains("/srv/other"));
        assert!(merged.contains("# keep this root"));
        assert!(merged.contains(r#"C:\\Users\\example"#));
        assert!(merged.contains(&managed_root.to_string_lossy().to_string()));
        assert!(merged.contains("exclude_tmpdir_env_var = true"));
        assert_eq!(
            ensure_managed_writable_root(&merged, &managed_root).expect("repeat merge"),
            merged
        );
    }

    #[cfg(unix)]
    #[test]
    fn managed_worktree_directory_is_private_and_rejects_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = TestDirectory::new("managed-worktree-directory");
        let worktrees = directory.path().join("codex").join("worktrees");
        ensure_private_directory(&worktrees).expect("create worktree directory");
        assert_eq!(
            fs::metadata(&worktrees)
                .expect("inspect worktree directory")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        ensure_private_directory(&worktrees).expect("repeat directory setup");

        let target = directory.path().join("target");
        let link = directory.path().join("link");
        fs::create_dir(&target).expect("create symlink target");
        symlink(&target, &link).expect("create symlink");
        assert!(ensure_private_directory(&link).is_err());
    }

    #[test]
    fn legacy_profile_detection_ignores_comments() {
        assert!(contains_legacy_profile_config("profile = \"teacher\"\n"));
        assert!(contains_legacy_profile_config("[profiles.teacher]\n"));
        assert!(!contains_legacy_profile_config("# profile = \"teacher\"\n"));
    }
}

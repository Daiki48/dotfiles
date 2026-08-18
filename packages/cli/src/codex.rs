use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::utils::{create_symlink, run_command};

const CODEX_FILES: &[(&str, &str)] = &[
    (".codex/AGENTS.md", ".codex/AGENTS.md"),
    (".codex/rules/default.rules", ".codex/rules/default.rules"),
    (
        ".codex/hooks/block_git_write.py",
        ".codex/hooks/block_git_write.py",
    ),
];

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

fn managed_assignments(template: &str, section: Option<&str>, keys: &[&str]) -> Vec<String> {
    let mut current_section = None;
    template
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('[') {
                current_section = Some(trimmed);
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

fn merge_managed_config(template: &str, existing: &str) -> String {
    let managed = managed_assignments(template, None, MANAGED_CONFIG_KEYS).join("\n");
    let managed_agents = managed_assignments(template, Some("[agents]"), MANAGED_AGENT_KEYS);

    let mut in_top_level = true;
    let mut in_workspace_sandbox = false;
    let mut in_agents = false;
    let mut workspace_sandbox_found = false;
    let mut agents_found = false;
    let mut preserved_lines = Vec::new();
    for line in existing.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_top_level = false;
            in_workspace_sandbox = trimmed == "[sandbox_workspace_write]";
            in_agents = trimmed == "[agents]";
            if in_workspace_sandbox {
                workspace_sandbox_found = true;
            }
            if in_agents {
                agents_found = true;
            }
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
        if in_workspace_sandbox && trimmed == "[sandbox_workspace_write]" {
            preserved_lines.push("network_access = true");
        }
        if in_agents && trimmed == "[agents]" {
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

    if preserved.is_empty() {
        format!("{managed}\n")
    } else {
        format!("{managed}\n\n{preserved}\n")
    }
}

fn replace_legacy_config(codex_dir: &Path) -> Result<()> {
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
        .as_secs();
    let backup_path = codex_dir.join(format!("config.toml.bak.legacy.{timestamp}"));

    println!(
        "\nLegacy Codex profile config detected. Backing up {} to {}.",
        config_path.display(),
        backup_path.display()
    );
    fs::rename(&config_path, &backup_path).with_context(|| {
        format!(
            "Failed to back up {} to {}",
            config_path.display(),
            backup_path.display()
        )
    })?;

    let dotfiles_path = std::env::current_dir().context("Failed to get current directory")?;
    let source_path = dotfiles_path.join(".codex/config.base.toml");
    fs::copy(&source_path, &config_path).with_context(|| {
        format!(
            "Failed to copy from {} to {}",
            source_path.display(),
            config_path.display()
        )
    })?;
    println!(
        "Replaced legacy config with the current base config: {}",
        config_path.display()
    );

    Ok(())
}

fn migrate_managed_config(codex_dir: &Path) -> Result<()> {
    let config_path = codex_dir.join("config.toml");
    let dotfiles_path = std::env::current_dir().context("Failed to get current directory")?;
    let template_path = dotfiles_path.join(CODEX_CONFIG_TEMPLATE);

    if !config_path.exists() {
        fs::copy(&template_path, &config_path).with_context(|| {
            format!(
                "Failed to copy from {} to {}",
                template_path.display(),
                config_path.display()
            )
        })?;
        println!("- Installed base config: {}", config_path.display());
        return Ok(());
    }

    let existing = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    let template = fs::read_to_string(&template_path)
        .with_context(|| format!("Failed to read {}", template_path.display()))?;
    let migrated = merge_managed_config(&template, &existing);
    if migrated == existing {
        println!("- Shared Codex settings are up to date.");
        return Ok(());
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock is before UNIX_EPOCH")?
        .as_secs();
    let backup_path = codex_dir.join(format!("config.toml.bak.automation.{timestamp}"));
    fs::copy(&config_path, &backup_path).with_context(|| {
        format!(
            "Failed to back up {} to {}",
            config_path.display(),
            backup_path.display()
        )
    })?;
    fs::write(&config_path, migrated)
        .with_context(|| format!("Failed to update {}", config_path.display()))?;
    println!(
        "- Updated shared Codex settings (backup: {}).",
        backup_path.display()
    );
    Ok(())
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

pub fn setup() -> Result<()> {
    println!("🧠 Setting up Codex CLI...\n");

    if !is_codex_installed() {
        println!("Codex CLI is not found.");
        codex_install()?;
    } else {
        println!("Codex CLI is already installed.");
    }

    codex_check()?;

    let home = home::home_dir().context("Cannot find home directory")?;
    let codex_dir = home.join(".codex");

    if !codex_dir.exists() {
        println!("\nCreating ~/.codex directory...");
        fs::create_dir_all(&codex_dir)?;
    }

    println!("\nLinking shared configuration files...");
    for (source, dest) in CODEX_FILES {
        create_symlink(source, dest)?;
    }

    println!("\nLinking shared skill directories...");
    for (source, dest) in CODEX_DIRS {
        create_symlink(source, dest)?;
    }

    replace_legacy_config(&codex_dir)?;
    archive_retired_profiles(&codex_dir)?;

    println!("\nMigrating shared Codex settings...");
    migrate_managed_config(&codex_dir)?;

    println!("\n✅ Codex CLI setup completed!");
    println!("\n💡 Next steps:");
    println!("   1. Run 'codex login' if authentication is not configured");
    println!("   2. Run 'codex' and trust hooks from '/hooks' if prompted");
    println!("   3. Run 'codex' (workspace-write + auto-review is the default)");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{contains_legacy_profile_config, merge_managed_config};

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
    fn legacy_profile_detection_ignores_comments() {
        assert!(contains_legacy_profile_config("profile = \"teacher\"\n"));
        assert!(contains_legacy_profile_config("[profiles.teacher]\n"));
        assert!(!contains_legacy_profile_config("# profile = \"teacher\"\n"));
    }
}

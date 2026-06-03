use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::utils::{copy_if_not_exists, create_symlink, run_command};

const CODEX_FILES: &[(&str, &str)] = &[
    (".codex/AGENTS.md", ".codex/AGENTS.md"),
    (".codex/teacher.config.toml", ".codex/teacher.config.toml"),
    (
        ".codex/autonomous.config.toml",
        ".codex/autonomous.config.toml",
    ),
    (".codex/rules/default.rules", ".codex/rules/default.rules"),
    (
        ".codex/hooks/block_git_write.py",
        ".codex/hooks/block_git_write.py",
    ),
];

// テンプレート専用ファイル (.codex/config.base.toml) を ~/.codex/config.toml へコピーする。
// dotfiles 内では `.codex/config.toml` を置かないことで、Codex の project config が
// profile (teacher/autonomous) を上書きしないようにしている。
const CODEX_COPY_FILES: &[(&str, &str)] = &[(".codex/config.base.toml", ".codex/config.toml")];

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

    println!("\nLinking configuration files...");
    for (source, dest) in CODEX_FILES {
        create_symlink(source, dest)?;
    }

    replace_legacy_config(&codex_dir)?;

    println!("\nCopying configuration files...");
    for (source, dest) in CODEX_COPY_FILES {
        copy_if_not_exists(source, dest)?;
    }

    println!("\n✅ Codex CLI setup completed!");
    println!("\n💡 Next steps:");
    println!("   1. Run 'codex login' if authentication is not configured");
    println!("   2. Run 'codex' and trust hooks from '/hooks' if prompted");
    println!("   3. Use teacher mode: codex --profile teacher");
    println!("   4. Use autonomous mode: codex --profile autonomous");

    Ok(())
}

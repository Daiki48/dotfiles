use anyhow::{Context, Result};
use std::fs;
use std::process::{Command, Stdio};

use crate::utils::{create_symlink, run_command};

const CODEX_FILES: &[(&str, &str)] = &[
    (".codex/AGENTS.md", ".codex/AGENTS.md"),
    (".codex/config.toml", ".codex/config.toml"),
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

    println!("\n✅ Codex CLI setup completed!");
    println!("\n💡 Next steps:");
    println!("   1. Run 'codex login' if authentication is not configured");
    println!("   2. Use teacher mode: codex --profile teacher");
    println!("   3. Use autonomous mode: codex --profile autonomous");

    Ok(())
}

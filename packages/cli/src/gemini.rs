use anyhow::{Context, Result};
use std::fs;
use std::process::{Command, Stdio};

use crate::utils::{create_symlink, run_command};

/// Gemini CLIがインストールされているか確認
fn is_gemini_installed() -> bool {
    Command::new("gemini")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// npmでGemini CLIをインストール
fn gemini_install() -> Result<()> {
    println!("Installing Gemini CLI via npm...");
    let mut cmd = Command::new("npm");
    cmd.args(["install", "-g", "@google/gemini-cli"]);
    run_command(cmd, "Failed to install Gemini CLI")
}

/// Gemini CLIのバージョンを表示
fn gemini_check() -> Result<()> {
    println!("\nGemini CLI version:");
    match Command::new("gemini").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Failed to get version info.");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => eprintln!("Failed to execute gemini: {}", e),
    }
    Ok(())
}

pub fn setup() -> Result<()> {
    println!("✨ Setting up Gemini CLI...\n");

    // 1. 未インストールならインストール
    if !is_gemini_installed() {
        println!("Gemini CLI is not found.");
        gemini_install()?;
    } else {
        println!("Gemini CLI is already installed.");
    }

    // 2. バージョン表示
    gemini_check()?;

    // 3. ディレクトリ作成
    let home = home::home_dir().context("Cannot find home directory")?;
    let gemini_dir = home.join(".gemini");

    if !gemini_dir.exists() {
        println!("\nCreating ~/.gemini directory...");
        fs::create_dir_all(&gemini_dir)?;
    }

    // 4. 設定ファイルのsymlink作成
    println!("\nLinking configuration files...");
    create_symlink(".gemini/settings.json", ".gemini/settings.json")?;

    println!("\n✅ Gemini CLI setup completed!");
    println!("\n💡 Next steps:");
    println!("   1. Get API key from: https://ai.google.dev/gemini-api/docs/api-key");
    println!("   2. Add to ~/.zshrc: export GEMINI_API_KEY=\"your-key\"");
    println!("   3. Run 'gemini' to start");

    Ok(())
}

use anyhow::{Context, Result};
use std::fs;
use std::process::{Command, Stdio};

use crate::utils::{create_symlink, run_command};

const CLAUDE_FILES: &[(&str, &str)] = &[
    (".claude/CLAUDE.md", ".claude/CLAUDE.md"),
    (".claude/settings.json", ".claude/settings.json"),
];

const CLAUDE_SKILLS: &[&str] = &[
    "axum-guide",
    "dioxus-guide",
    "leptos-guide",
    "rusqlite-guide",
    "rust-fullstack",
    "snipmind-arch",
    "sqlx-postgres",
];

/// Claude Codeがインストールされているか確認
fn is_claude_installed() -> bool {
    Command::new("claude")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// 公式インストーラーでClaude Codeをインストール
fn claude_install() -> Result<()> {
    println!("Installing Claude Code via official installer...");
    let mut cmd = Command::new("bash");
    cmd.args(["-c", "curl -fsSL https://claude.ai/install.sh | bash"]);
    run_command(cmd, "Failed to install Claude Code")
}

/// Claude Codeのバージョンを表示
fn claude_check() -> Result<()> {
    println!("\nClaude Code version:");
    match Command::new("claude").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Failed to get version info.");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => eprintln!("Failed to execute claude: {}", e),
    }
    Ok(())
}

pub fn setup() -> Result<()> {
    println!("🤖 Setting up Claude Code...\n");

    // 1. 未インストールならインストール
    if !is_claude_installed() {
        println!("Claude Code is not found.");
        claude_install()?;
    } else {
        println!("Claude Code is already installed.");
    }

    // 2. バージョン表示
    claude_check()?;

    // 3. ディレクトリ作成
    let home = home::home_dir().context("Cannot find home directory")?;
    let claude_dir = home.join(".claude");
    let skills_dir = claude_dir.join("skills");

    if !claude_dir.exists() {
        println!("\nCreating ~/.claude directory...");
        fs::create_dir_all(&claude_dir)?;
    }
    if !skills_dir.exists() {
        fs::create_dir_all(&skills_dir)?;
    }

    // 4. 設定ファイルのsymlink作成
    println!("\nLinking configuration files...");
    for (source, dest) in CLAUDE_FILES {
        create_symlink(source, dest)?;
    }

    // 5. スキルのsymlink作成
    println!("\nLinking skills...");
    for skill in CLAUDE_SKILLS {
        let source = format!(".claude/skills/{}", skill);
        let dest = format!(".claude/skills/{}", skill);
        create_symlink(&source, &dest)?;
    }

    println!("\n✅ Claude Code setup completed!");
    println!("\n💡 Next step: Run 'claude' to authenticate if needed.");

    Ok(())
}

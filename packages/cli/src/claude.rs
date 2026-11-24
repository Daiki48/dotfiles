use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::utils::{create_symlink, run_command};

// skills/, agents/ はディレクトリ単位でsymlinkし、中身の個別管理を不要にする
const CLAUDE_DIRS: &[(&str, &str)] = &[
    (".claude/skills", ".claude/skills"),
    (".claude/agents", ".claude/agents"),
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

// 個人設定では共通規約に必要な項目だけを更新する。モデルや権限の選択は保持する。
fn merge_settings(current: &mut Value, template: &Value) -> Result<()> {
    fn object(value: &mut Value) -> Result<&mut serde_json::Map<String, Value>> {
        value
            .as_object_mut()
            .context("Claude Code の設定項目にはオブジェクトが必要です")
    }
    for path in [
        &["attribution", "commit"][..],
        &["attribution", "pr"],
        &[
            "pluginConfigs",
            "cc-plugin-agents-md@builtin",
            "options",
            "instructionFiles",
        ],
    ] {
        let (key, parents) = path.split_last().unwrap();
        let mut node = &mut *current;
        let mut source = template;
        for parent in parents {
            node = object(node)?.entry(*parent).or_insert_with(|| json!({}));
            source = &source[parent];
        }
        object(node)?.insert(key.to_string(), source[key].clone());
    }
    let hooks = object(current)?.entry("hooks").or_insert_with(|| json!({}));
    for event in ["PreToolUse", "Stop"] {
        let list = object(hooks)?
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("Claude Code の hooks には配列が必要です")?;
        let hook = &template["hooks"][event][0];
        if !list.contains(hook) {
            list.push(hook.clone());
        }
    }
    Ok(())
}

fn install_settings(directory: &Path) -> Result<()> {
    let relative = ".claude/settings.json";
    let source = std::env::current_dir()?.join(relative);
    let path = directory.join("settings.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return create_symlink(relative, relative);
        }
        Err(e) => return Err(e.into()),
    };
    if metadata.is_symlink() && fs::canonicalize(&path)? == fs::canonicalize(&source)? {
        return Ok(());
    }
    crate::codex::verify_current_user_regular_file(&path, &metadata)?;
    let original = fs::read_to_string(&path)?;
    let current: Value = serde_json::from_str(&original)?;
    let mut merged = current.clone();
    merge_settings(&mut merged, &serde_json::from_slice(&fs::read(source)?)?)?;
    if current != merged {
        crate::codex::replace_personal_config(
            &path,
            &original,
            &serde_json::to_vec_pretty(&merged)?,
            metadata.permissions(),
        )?;
    }
    Ok(())
}

fn install_instructions(directory: &Path) -> Result<()> {
    let destination = ".claude/CLAUDE.md";
    let shared = ".codex/AGENTS.md";
    let path = directory.join("CLAUDE.md");
    let root = std::env::current_dir()?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return create_symlink(shared, destination);
        }
        Err(e) => return Err(e.into()),
    };
    if metadata.is_symlink() {
        let target = fs::canonicalize(&path)?;
        anyhow::ensure!(
            target == root.join(shared) || target == root.join(destination),
            "Claude Code の規約に管理外のリンクが設定されています"
        );
        return Ok(());
    }
    crate::codex::verify_current_user_regular_file(&path, &metadata)?;
    let original = fs::read_to_string(&path)?;
    let import = "@~/.codex/AGENTS.md";
    if !original.lines().any(|line| line.trim() == import) {
        crate::codex::replace_personal_config(
            &path,
            &original,
            format!("{original}\n\n{import}\n").as_bytes(),
            metadata.permissions(),
        )?;
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

    // 3. ~/.claude ディレクトリ作成
    let home = crate::codex::setup_home_dir()?;
    let claude_dir = home.join(".claude");
    if !claude_dir.exists() {
        println!("\nCreating ~/.claude directory...");
        fs::create_dir_all(&claude_dir)?;
    }

    // 4. 設定ファイルのsymlink作成
    println!("\nLinking configuration files...");
    anyhow::ensure!(
        !fs::symlink_metadata(&claude_dir)?.is_symlink(),
        "Claude Code の設定ディレクトリがリンクです"
    );
    anyhow::ensure!(
        home.join(".codex/hooks/block-git-write").is_file()
            && home.join(".codex/AGENTS.md").is_file(),
        "先に Codex セットアップで共通 helper と規約を導入してください"
    );
    install_instructions(&claude_dir)?;
    install_settings(&claude_dir)?;
    if Path::new(".claude/settings.local.json").is_file() {
        create_symlink(".claude/settings.local.json", ".claude/settings.local.json")?;
    }

    // 5. ディレクトリのsymlink作成（skills/, agents/）
    println!("\nLinking directories...");
    for (source, dest) in CLAUDE_DIRS {
        create_symlink(source, dest)?;
    }

    println!("\n✅ Claude Code setup completed!");
    println!("\n💡 Next step: Run 'claude' to authenticate if needed.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_managed_keys_once_and_keeps_personal_settings() {
        let template: Value =
            serde_json::from_str(include_str!("../../../.claude/settings.json")).unwrap();
        let local = json!({"matcher":"Read","hooks":[]});
        let mut result = json!({"model":"example","permissions":{"deny":["Bash(example)"]},"hooks":{"PreToolUse":[local.clone()]}});
        merge_settings(&mut result, &template).unwrap();
        let once = result.clone();
        merge_settings(&mut result, &template).unwrap();
        assert_eq!(result, once);
        assert_eq!(result["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(result["hooks"]["PreToolUse"][0], local);
        assert_eq!(result["model"], "example");
        assert_eq!(result["permissions"]["deny"], json!(["Bash(example)"]));
        assert_eq!(result["attribution"], json!({"commit":"","pr":""}));
    }
}

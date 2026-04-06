use anyhow::{Context, Result};
use std::process::{Command, Stdio};

use crate::common::Distro;
use crate::utils::{create_symlink, run_command};

const NEOVIM_REPO_URL: &str = "https://github.com/neovim/neovim.git";

/// Neovimの初回セットアップ（指定タグでビルド・インストール）
pub fn setup(distro: &Distro, tag: &str) -> Result<()> {
    verify_tag_exists(tag)?;

    if is_nvim_installed() {
        println!("Neovimは既にインストールされています。");
        print_nvim_version();
        println!(
            "バージョンを変更する場合は `neovim-update --tag {}` を使用してください。",
            tag
        );
    } else {
        println!("Neovimが見つかりません。ソースからビルドします...");
        build_from_source(distro, tag)?;
    }

    println!("\nNeovim設定のシンボリックリンクを作成します...");
    create_symlink(".config/nvim", ".config/nvim")?;
    Ok(())
}

/// Neovimを指定タグにアップデート（既にインストール済みであること前提）
pub fn update(distro: &Distro, tag: &str) -> Result<()> {
    if !is_nvim_installed() {
        anyhow::bail!(
            "Neovimがインストールされていません。先に `neovim --tag {}` でセットアップしてください。",
            tag
        );
    }

    println!("------ 現在のNeovimバージョン ------");
    print_nvim_version();
    println!("------------------------------------");

    verify_tag_exists(tag)?;

    println!("\nタグ {} にアップデートします...", tag);
    build_from_source(distro, tag)?;

    println!("\n------ アップデート後のNeovimバージョン ------");
    print_nvim_version();
    println!("----------------------------------------------");

    Ok(())
}

/// 現在のNeovimバージョンを表示
fn print_nvim_version() {
    match Command::new("nvim").arg("-v").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("バージョン情報の取得に失敗しました:");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            eprintln!("'nvim -v' の実行に失敗しました: {}", e);
        }
    }
}

/// リモートリポジトリに指定タグが存在するか検証
fn verify_tag_exists(tag: &str) -> Result<()> {
    println!("リモートリポジトリでタグ {} の存在を確認中...", tag);

    let output = Command::new("git")
        .args([
            "ls-remote",
            "--tags",
            NEOVIM_REPO_URL,
            &format!("refs/tags/{}", tag),
        ])
        .output()
        .context("git ls-remote の実行に失敗しました")?;

    if !output.status.success() {
        anyhow::bail!("git ls-remote の実行に失敗しました");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        anyhow::bail!(
            "タグ '{}' はNeovimリポジトリに存在しません。正しいタグ名を指定してください（例: v0.12.0）",
            tag
        );
    }

    println!("タグ {} の存在を確認しました。", tag);
    Ok(())
}

/// Neovimがインストール済みか確認
fn is_nvim_installed() -> bool {
    Command::new("nvim")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// 指定タグでNeovimをソースビルド
fn build_from_source(distro: &Distro, tag: &str) -> Result<()> {
    println!("Installing build dependencies...");

    let (mut cmd, deps) = match distro {
        Distro::Ubuntu => {
            let mut cmd = Command::new("sudo");
            cmd.arg("apt").arg("install").arg("-y");
            let deps = vec![
                "ninja-build",
                "gettext",
                "libtool",
                "libtool-bin",
                "autoconf",
                "automake",
                "cmake",
                "g++",
                "pkg-config",
                "unzip",
                "curl",
                "doxygen",
            ];
            (cmd, deps)
        }
        Distro::Fedora => {
            let mut cmd = Command::new("sudo");
            cmd.arg("dnf").arg("install").arg("-y");
            let deps = vec![
                "ninja-build",
                "libtool",
                "autoconf",
                "automake",
                "cmake",
                "gcc-c++",
                "gettext",
                "doxygen",
                "unzip",
                "curl",
                "pkg-config",
            ];
            (cmd, deps)
        }
    };
    cmd.args(&deps);
    run_command(cmd, "Failed to install dependencies.")?;

    let home_dir = home::home_dir().context("Failed to get home directory")?;
    let repo_path = home_dir.join("neovim");

    let need_clean = repo_path.exists();

    if repo_path.exists() {
        println!(
            "Neovimリポジトリが {:?} に見つかりました。タグ {} に切り替えます...",
            repo_path, tag
        );

        // 指定タグのコミットのみをfetch（shallow clone対応）
        let mut cmd_fetch = Command::new("git");
        cmd_fetch
            .args([
                "fetch",
                "--depth",
                "1",
                "origin",
                &format!("+refs/tags/{}:refs/tags/{}", tag, tag),
            ])
            .current_dir(&repo_path);
        run_command(cmd_fetch, "originからのfetchに失敗しました")?;

        let mut cmd_checkout = Command::new("git");
        cmd_checkout
            .args(["checkout", &format!("refs/tags/{}", tag)])
            .current_dir(&repo_path);
        run_command(
            cmd_checkout,
            &format!("タグ {} のチェックアウトに失敗しました", tag),
        )?;
    } else {
        println!("Neovimリポジトリを {:?} にクローンします...", repo_path);

        let repo_path_str = repo_path.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "リポジトリパスに無効なUnicode文字が含まれています: {}",
                repo_path.display()
            )
        })?;

        let mut cmd_clone = Command::new("git");
        cmd_clone.args([
            "clone",
            "--branch",
            tag,
            "--depth",
            "1",
            NEOVIM_REPO_URL,
            repo_path_str,
        ]);
        run_command(cmd_clone, "Neovimリポジトリのクローンに失敗しました")?;
    }

    // 初回クローン時はビルド成果物が存在しないためスキップ
    if need_clean {
        println!("以前のビルド成果物と依存キャッシュをクリーンアップ中...");
        let mut cmd_clean = Command::new("make");
        cmd_clean.arg("distclean").current_dir(&repo_path);
        run_command(cmd_clean, "'make distclean' の実行に失敗しました")?;
    }

    println!("Neovimをビルド中...");
    let mut cmd_build = Command::new("make");
    cmd_build
        .arg("CMAKE_BUILD_TYPE=Release")
        .current_dir(&repo_path);
    run_command(cmd_build, "Neovimのビルドに失敗しました")?;

    println!("Neovimをインストール中...(sudoパスワードが必要な場合があります)");
    let mut cmd_install = Command::new("sudo");
    cmd_install
        .arg("make")
        .arg("install")
        .current_dir(&repo_path);
    run_command(cmd_install, "Neovimのインストールに失敗しました")?;

    println!(
        "ビルドとインストールが完了しました。リポジトリは {:?} に保持されています",
        repo_path
    );

    Ok(())
}

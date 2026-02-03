use anyhow::{Context, Result};
use std::process::{Command, Stdio};

use crate::common::Distro;
use crate::utils::{create_symlink, run_command};

/// tmuxのセットアップを実行
pub fn setup(distro: &Distro) -> Result<()> {
    // tmuxのインストール確認・インストール
    if !is_tmux_installed() {
        println!("tmux is not found.");
        println!("Starting tmux install...");
        tmux_install(distro)?;
    } else {
        println!("tmux is already installed.");
    }

    tmux_check()?;

    // TPM（Tmux Plugin Manager）のインストール
    println!("\nChecking TPM (Tmux Plugin Manager)...");
    install_tpm()?;

    // シンボリックリンクの作成
    println!("\nSetting up symbolic link for tmux config...");
    create_symlink(".config/tmux/tmux.conf", ".config/tmux/tmux.conf")?;

    println!("\n📝 Next steps:");
    println!("   1. Start tmux: tmux");
    println!("   2. Install plugins: Ctrl+g → I (capital I)");

    Ok(())
}

/// tmuxがインストールされているか確認
fn is_tmux_installed() -> bool {
    Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// tmuxをインストール
fn tmux_install(distro: &Distro) -> Result<()> {
    match distro {
        Distro::Ubuntu => {
            let mut cmd = Command::new("sudo");
            cmd.arg("apt").arg("install").arg("-y").arg("tmux");
            run_command(cmd, "Failed to install tmux via apt.")?;
        }
        Distro::Fedora => {
            let mut cmd = Command::new("sudo");
            cmd.arg("dnf").arg("install").arg("-y").arg("tmux");
            run_command(cmd, "Failed to install tmux via dnf.")?;
        }
    }
    Ok(())
}

/// TPM（Tmux Plugin Manager）をインストール
fn install_tpm() -> Result<()> {
    let home_path = home::home_dir().context("Failed to get home directory")?;
    let tpm_path = home_path.join(".config/tmux/plugins/tpm");

    if tpm_path.exists() {
        println!("TPM is already installed at {}", tpm_path.display());
        return Ok(());
    }

    println!("Installing TPM...");

    // 親ディレクトリを作成
    let plugins_dir = home_path.join(".config/tmux/plugins");
    std::fs::create_dir_all(&plugins_dir).with_context(|| {
        format!(
            "Failed to create plugins directory: {}",
            plugins_dir.display()
        )
    })?;

    // TPMをgit clone
    let mut cmd = Command::new("git");
    cmd.arg("clone")
        .arg("https://github.com/tmux-plugins/tpm")
        .arg(&tpm_path);
    run_command(cmd, "Failed to clone TPM repository.")?;

    println!("TPM installed successfully at {}", tpm_path.display());
    Ok(())
}

/// tmuxのバージョン確認
fn tmux_check() -> Result<()> {
    println!("\n------ Current tmux Version ------");
    match Command::new("tmux").arg("-V").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Failed to get tmux version info. Stderr:");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            eprintln!("Failed to execute 'tmux -V': {}", e);
        }
    }
    println!("-----------------------------------");
    Ok(())
}

use anyhow::Result;
use std::process::{Command, Stdio};

use crate::common::Distro;
use crate::utils::{create_symlink, run_command};

/// Zellijのセットアップを実行
pub fn setup(distro: &Distro) -> Result<()> {
    if !is_zellij_installed() {
        println!("Zellij is not found.");
        println!("Starting Zellij install...");
        zellij_install(distro)?;
    } else {
        println!("Zellij is already installed.");
    }

    zellij_check()?;

    println!("\nSetting up symbolic link for Zellij config...");
    create_symlink(".config/zellij", ".config/zellij")?;
    Ok(())
}

/// Zellijがインストールされているか確認
fn is_zellij_installed() -> bool {
    Command::new("zellij")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Zellijをインストール
fn zellij_install(distro: &Distro) -> Result<()> {
    match distro {
        Distro::Ubuntu => {
            // Ubuntu: cargoでインストール（公式推奨）
            let mut cmd = Command::new("cargo");
            cmd.arg("install").arg("zellij");
            run_command(cmd, "Failed to install Zellij via cargo.")?;
        }
        Distro::Fedora => {
            // Fedora: dnfでインストール可能
            let mut cmd = Command::new("sudo");
            cmd.arg("dnf")
                .arg("install")
                .arg("-y")
                .arg("zellij");
            run_command(cmd, "Failed to install Zellij via dnf.")?;
        }
    }
    Ok(())
}

/// Zellijのバージョン確認
fn zellij_check() -> Result<()> {
    println!("\n------ Current Zellij Version ------");
    match Command::new("zellij").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Failed to get Zellij version info. Stderr:");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            eprintln!("Failed to execute 'zellij --version': {}", e);
        }
    }
    println!("-------------------------------------");
    Ok(())
}

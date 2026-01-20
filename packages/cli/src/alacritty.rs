use anyhow::Result;
use std::process::{Command, Stdio};

use crate::common::Distro;
use crate::utils::{create_symlink, run_command};

/// Alacrittyのセットアップを実行
pub fn setup(distro: &Distro) -> Result<()> {
    if !is_alacritty_installed() {
        println!("Alacritty is not found.");
        println!("Starting Alacritty install...");
        alacritty_install(distro)?;
    } else {
        println!("Alacritty is already installed.");
    }

    alacritty_check()?;

    println!("\nSetting up symbolic link for Alacritty config...");
    create_symlink(".config/alacritty", ".config/alacritty")?;
    Ok(())
}

/// Alacrittyがインストールされているか確認
fn is_alacritty_installed() -> bool {
    Command::new("alacritty")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Alacrittyをインストール
fn alacritty_install(distro: &Distro) -> Result<()> {
    let mut cmd = Command::new("sudo");

    match distro {
        Distro::Ubuntu => {
            // Ubuntu: PPAを追加してインストール
            cmd.arg("apt")
                .arg("install")
                .arg("-y")
                .arg("alacritty");
        }
        Distro::Fedora => {
            cmd.arg("dnf")
                .arg("install")
                .arg("-y")
                .arg("alacritty");
        }
    }
    run_command(cmd, "Failed to install Alacritty.")?;
    Ok(())
}

/// Alacrittyのバージョン確認
fn alacritty_check() -> Result<()> {
    println!("\n------ Current Alacritty Version ------");
    match Command::new("alacritty").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Failed to get Alacritty version info. Stderr:");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            eprintln!("Failed to execute 'alacritty --version': {}", e);
        }
    }
    println!("----------------------------------------");
    Ok(())
}

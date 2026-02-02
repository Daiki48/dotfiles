use anyhow::Result;
use std::process::{Command, Stdio};

use crate::common::Distro;
use crate::utils::{create_symlink, run_command};

/// Ghosttyのセットアップを実行
pub fn setup(distro: &Distro) -> Result<()> {
    if !is_ghostty_installed() {
        println!("Ghostty is not found.");
        println!("Starting Ghostty install...");
        ghostty_install(distro)?;
    } else {
        println!("Ghostty is already installed.");
    }

    ghostty_check()?;

    println!("\nSetting up symbolic link for Ghostty config...");
    create_symlink(".config/ghostty", ".config/ghostty")?;
    Ok(())
}

/// Ghosttyがインストールされているか確認
fn is_ghostty_installed() -> bool {
    Command::new("ghostty")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Ghosttyをインストール
fn ghostty_install(distro: &Distro) -> Result<()> {
    match distro {
        Distro::Ubuntu => {
            // Ubuntu/Linux Mint: コミュニティ提供のインストールスクリプトを使用
            // https://github.com/mkasberg/ghostty-ubuntu
            println!("Installing Ghostty via ghostty-ubuntu installer...");
            println!("This will download and install the .deb package for your system.");

            let mut cmd = Command::new("bash");
            cmd.arg("-c").arg(
                "curl -fsSL https://raw.githubusercontent.com/mkasberg/ghostty-ubuntu/HEAD/install.sh | bash",
            );
            run_command(cmd, "Failed to install Ghostty.")?;
        }
        Distro::Fedora => {
            // Fedora: dnfでインストール可能か確認（COPR等）
            println!("Installing Ghostty via dnf...");
            let mut cmd = Command::new("sudo");
            cmd.arg("dnf").arg("install").arg("-y").arg("ghostty");
            run_command(cmd, "Failed to install Ghostty.")?;
        }
    }
    Ok(())
}

/// Ghosttyのバージョン確認
fn ghostty_check() -> Result<()> {
    println!("\n------ Current Ghostty Version ------");
    match Command::new("ghostty").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Failed to get Ghostty version info. Stderr:");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            eprintln!("Failed to execute 'ghostty --version': {}", e);
        }
    }
    println!("--------------------------------------");
    Ok(())
}

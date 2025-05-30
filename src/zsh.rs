use anyhow::Result;
use std::process::{Command, Stdio};

use crate::utils::{create_symlink, run_command};

/// Execute setup for Zsh
pub fn setup() -> Result<()> {
    if !is_zsh_installed() {
        println!("Zsh is not found.");
        println!("Starting zsh install...");
        zsh_install()?;
        zsh_check()?;
    } else {
        println!("Zsh is already installed.");
        zsh_check()?;
    }

    println!("\nSetting up symbolic link for .zshrc...");
    create_symlink(".zshrc", ".zshrc")?;

    println!("\nSetting up symbolic link for .zsh directory...");
    create_symlink(".zsh", ".zsh")?;

    Ok(())
}

/// Checking Zsh version
fn is_zsh_installed() -> bool {
    Command::new("zsh")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn zsh_install() -> Result<()> {
    let mut cmd = Command::new("sudo");
    cmd.arg("apt").arg("install").arg("zsh");
    run_command(cmd, "Failed to install dependencies.")?;

    Ok(())
}

fn zsh_check() -> Result<()> {
    println!("\n------ Current Zsh Version ------");
    match Command::new("zsh").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Failed to get Neovim version info. Stderr:");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            eprintln!("Failed to execute 'zsh -version': {}", e);
        }
    }
    println!("---------------------------------");
    Ok(())
}

use anyhow::{Context, Result};
use std::env;
use std::process::{Command, Stdio};
use std::str;

use crate::utils::{create_symlink, run_command};

/// Execute setup for Zsh
pub fn setup() -> Result<()> {
    if !is_zsh_installed() {
        println!("Zsh is not found.");
        println!("Starting zsh install...");
        zsh_install()?;
    } else {
        println!("Zsh is already installed.");
    }

    zsh_check()?;
    echo_shell()?;

    println!("\nSetting up symbolic link for .zshrc...");
    create_symlink(".zshrc", ".zshrc")?;

    println!("\nSetting up symbolic link for .zsh directory...");
    create_symlink(".zsh", ".zsh")?;

    println!("\nAttempting to change the default shell to Zsh...");
    match change_default_shell_to_zsh() {
        Ok(changed) => {
            if changed {
                println!("Default shell change command executed successfully.");
                println!(
                    "IMPORTANT: Please close and reopen your terminal, or log out and log back in for the change to take effect."
                );
            } else {
                println!(
                    "Default shell is already Zsh or Zsh path not found. No changes made to default shell setting."
                );
            }
        }
        Err(e) => {
            eprintln!("Failed to attempt changing default shell: {}", e);
            println!("You may need to change it manually using: chsh -s $(which zsh)");
        }
    }

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
    cmd.arg("apt").arg("install").arg("-y").arg("zsh");
    run_command(cmd, "Failed to install zsh.")?;
    Ok(())
}

fn zsh_check() -> Result<()> {
    println!("\n------ Current Zsh Version ------");
    match Command::new("zsh").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Failed to get Zsh version info. Stderr:");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            eprintln!("Failed to execute 'zsh --version': {}", e);
        }
    }
    println!("---------------------------------");
    Ok(())
}

fn echo_shell() -> Result<()> {
    println!("\n------ Current SHELL ------");
    match env::var("SHELL") {
        Ok(shell_path) => {
            println!("{}", shell_path);
        }
        Err(e) => {
            eprintln!("Failed to get SHELL environment variable: {}", e);
        }
    }
    println!("---------------------------");
    Ok(())
}

fn get_zsh_path() -> Result<Option<String>> {
    let output = Command::new("which")
        .arg("zsh")
        .output()
        .context("Failed to execute 'which zsh'")?;

    if output.status.success() {
        let path = str::from_utf8(&output.stdout)
            .context("Failed to parse 'which zsh' output as UTF-8")?
            .trim();
        if path.is_empty() {
            Ok(None)
        } else {
            Ok(Some(path.to_string()))
        }
    } else {
        Ok(None)
    }
}

fn change_default_shell_to_zsh() -> Result<bool> {
    let current_shell_env = env::var("SHELL")?;

    match get_zsh_path() {
        Ok(Some(zsh_path)) => {
            if current_shell_env == zsh_path {
                println!("Default shell is already set to: {}", zsh_path);
                return Ok(false);
            }

            println!("Found zsh at: {}", zsh_path);
            println!(
                "Attempting to set it as the default shell using 'chsh -s {}'...",
                zsh_path
            );
            println!("You may be prompted for your password.");

            let mut cmd = Command::new("chsh");
            cmd.arg("-s").arg(&zsh_path);

            let status = cmd
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .context(format!("Failed to execute chsh -s {}", zsh_path))?;

            if status.success() {
                Ok(true)
            } else {
                anyhow::bail!(
                    "'chsh -s {}' command failed with status: {}",
                    zsh_path,
                    status
                );
            }
        }
        Ok(None) => {
            println!("Zsh not found in PATH. Cannot change default shell.");
            Ok(false)
        }
        Err(e) => Err(e).context("Failed to determine Zsh path"),
    }
}

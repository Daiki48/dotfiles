use anyhow::Result;
use std::process::{Command, Stdio};

use crate::common::Distro;
use crate::utils::{create_symlink, run_command};

/// Execute setup for Wezterm
pub fn setup(distro: &Distro) -> Result<()> {
    if !is_wezterm_installed() {
        println!("Wezterm is not found.");
        println!("Starting wezterm install...");
        wezterm_install(distro)?;
    } else {
        println!("Wezterm is already installed.");
    }

    wezterm_check()?;

    println!("\nSetting up symbolic link for Wezterm config...");
    create_symlink(".config/wezterm", ".config/wezterm")?;
    Ok(())
}

/// Checking Wezterm version
fn is_wezterm_installed() -> bool {
    Command::new("wezterm")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn wezterm_install(distro: &Distro) -> Result<()> {
    let mut cmd = Command::new("sudo");

    match distro {
        Distro::Ubuntu => {
            // cmd.arg("curl")
            //     .arg("-fsSL")
            //     .arg("https://apt.fury.io/wez/gpg.key");
            // cmd.arg("gpg")
            //     .arg("--yes")
            //     .arg("--dearmor")
            //     .arg("-o")
            //     .arg("/usr/share/keyrings/wezterm-fury.gpg");
            // cmd.arg("echo").arg("'deb [signed-by=/usr/share/keyrings/wezterm-fury.gpg] https://apt.fury.io/wez/ * *'");
            // cmd.arg("tee").arg("/etc/apt/sources.list.d/wezterm.list");
            // cmd.arg("chmod")
            //     .arg("644")
            //     .arg("/usr/share/keyrings/wezterm-fury.gpg");
            cmd.arg("apt").arg("update");
            cmd.arg("apt").arg("install").arg("wezterm");
        }
        Distro::Fedora => {
            cmd.arg("dnf").arg("install").arg("-y").arg("https://github.com/wezterm/wezterm/releases/download/20240203-110809-5046fc22/wezterm-20240203_110809_5046fc22-1.fedora42.x86_64.rpm");
        }
    }
    run_command(cmd, "Failed to install wezterm.")?;
    Ok(())
}

fn wezterm_check() -> Result<()> {
    println!("\n------ Current Wezterm Version ------");
    match Command::new("wezterm").arg("--version").output() {
        Ok(output) => {
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                eprintln!("Failed to get Wezterm version info. Stderr:");
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Err(e) => {
            eprintln!("Failed to execute 'wezterm --version': {}", e);
        }
    }
    println!("---------------------------------");
    Ok(())
}

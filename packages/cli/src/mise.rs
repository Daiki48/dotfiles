use anyhow::Result;
use std::process::{Command, Stdio};

use crate::common::Distro;
use crate::utils::run_command;

/// miseのセットアップを実行
pub fn setup(distro: &Distro, tools: &[String]) -> Result<()> {
    if !is_mise_installed() {
        println!("mise is not found.");
        println!("Starting mise install...");
        mise_install(distro)?;
    } else {
        println!("mise is already installed.");
    }

    mise_check()?;

    if tools.is_empty() {
        println!("\nNo global tools were specified. Skipping runtime installation.");
    } else {
        install_global_tools(tools)?;
    }

    println!("\n✅ mise setup completed!");
    println!("\n💡 Restart the shell to enable mise activation from ~/.zshrc.");
    println!("   Project example: mise use node@lts python@latest deno@latest");

    Ok(())
}

/// miseがインストールされているか確認
fn is_mise_installed() -> bool {
    Command::new("mise")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// mise公式ドキュメントで推奨されるパッケージリポジトリからインストール
fn mise_install(distro: &Distro) -> Result<()> {
    match distro {
        Distro::Ubuntu => {
            let mut install_extrepo = Command::new("sudo");
            install_extrepo.args(["apt", "install", "-y", "extrepo"]);
            run_command(
                install_extrepo,
                "Failed to install extrepo for the mise repository.",
            )?;

            let mut enable_repository = Command::new("sudo");
            enable_repository.args(["extrepo", "enable", "mise"]);
            run_command(
                enable_repository,
                "Failed to enable the recommended mise repository.",
            )?;

            let mut update = Command::new("sudo");
            update.args(["apt", "update"]);
            run_command(update, "Failed to update apt metadata.")?;

            let mut install = Command::new("sudo");
            install.args(["apt", "install", "-y", "mise"]);
            run_command(install, "Failed to install mise via apt.")?;
        }
        Distro::Fedora => {
            let mut enable_repository = Command::new("sudo");
            enable_repository.args(["dnf", "copr", "enable", "-y", "jdxcode/mise"]);
            run_command(
                enable_repository,
                "Failed to enable the recommended mise COPR repository.",
            )?;

            let mut install = Command::new("sudo");
            install.args(["dnf", "install", "-y", "mise"]);
            run_command(install, "Failed to install mise via dnf.")?;
        }
    }

    Ok(())
}

/// miseのバージョンを表示
fn mise_check() -> Result<()> {
    println!("\n------ Current mise Version ------");
    let mut command = Command::new("mise");
    command.arg("--version");
    run_command(command, "Failed to get mise version.")?;
    println!("----------------------------------");
    Ok(())
}

/// 指定されたツールをグローバル設定へ追加してインストール
fn install_global_tools(tools: &[String]) -> Result<()> {
    println!("\nInstalling global tools with mise: {}", tools.join(" "));

    let mut command = Command::new("mise");
    command.arg("use").arg("--global").args(tools);
    run_command(command, "Failed to install global tools with mise.")?;

    Ok(())
}

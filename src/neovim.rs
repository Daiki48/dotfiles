use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::process::{Command, Stdio};

const NEOVIM_REPO_URL: &str = "https://github.com/neovim/neovim.git";
const NEOVIM_BRANCH: &str = "release-0.11";

/// Execute setup for Neovim
pub fn setup() -> Result<()> {
    if !is_nvim_installed() {
        println!("Neovim is not found.");
        println!("Cloning and building Neovim from source...");
        build_from_source()?;
    } else {
        println!("Neovim is already installed.");
        println!("\n------ Current Neovim Version ------");
        match Command::new("nvim").arg("-v").output() {
            Ok(output) => {
                if output.status.success() {
                    print!("{}", String::from_utf8_lossy(&output.stdout));
                } else {
                    eprintln!("Failed to get Neovim version info. Stderr:");
                    eprint!("{}", String::from_utf8_lossy(&output.stderr));
                }
            }
            Err(e) => {
                eprintln!("Failed to execute 'nvim -v': {}", e);
            }
        }
        println!("------------------------------------");
    }

    println!("\nSetting up symbolic link for Neovim config...");
    create_symlink()?;
    Ok(())
}

/// Step1. Git clone from Neovim source
/// Step2. Building
/// Step3. Installing
fn is_nvim_installed() -> bool {
    Command::new("nvim")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn build_from_source() -> Result<()> {
    println!("Installing build dependencies...");
    let deps = [
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
    let mut cmd = Command::new("sudo");
    cmd.arg("apt").arg("install").arg("-y").args(deps);
    run_command(cmd, "Failed to install dependencies.")?;

    let home_dir = home::home_dir().context("Failed to get home directory")?;
    let repo_path = home_dir.join("neovim");

    if repo_path.exists() {
        println!("Neovim repository found at {:?}. Updating...", repo_path);

        let mut cmd_fetch = Command::new("git");
        cmd_fetch.arg("fetch").arg("origin").current_dir(&repo_path);
        run_command(cmd_fetch, "Failed to fetch from origin")?;

        let mut cmd_checkout = Command::new("git");
        cmd_checkout
            .arg("checkout")
            .arg(NEOVIM_BRANCH)
            .current_dir(&repo_path);
        run_command(
            cmd_checkout,
            &format!("Failed to checkout branch {}", NEOVIM_BRANCH),
        )?;

        let mut cmd_pull = Command::new("git");
        cmd_pull
            .args(["pull", "origin", NEOVIM_BRANCH])
            .current_dir(&repo_path);
        run_command(cmd_pull, "Failed to fetch from origin")?;
    } else {
        println!("Cloning Neovim repository to {:?}...", repo_path);

        let repo_path_str = repo_path.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "The repository path contains invalid Unicode characters: {}",
                repo_path.display()
            )
        })?;

        let mut cmd_clone = Command::new("git");
        cmd_clone.args([
            "clone",
            "--branch",
            NEOVIM_BRANCH,
            "--depth",
            "1",
            NEOVIM_REPO_URL,
            repo_path_str,
        ]);
        run_command(cmd_clone, "Failed to clone Neovim repository")?;
    }

    println!("Cleaning up previous build artifacts...");
    let mut cmd_clean = Command::new("make");
    cmd_clean.arg("clean").current_dir(&repo_path);
    run_command(cmd_clean, "Failed to run 'make clean'")?;

    println!("Building Neovim...");
    let mut cmd_build = Command::new("make");
    cmd_build
        .arg("CMAKE_BUILD_TYPE=Release")
        .current_dir(&repo_path);
    run_command(cmd_build, "Failed to build Neovim")?;

    println!("Installing Neovim...(sudo password may be required)");
    let mut cmd_install = Command::new("sudo");
    cmd_install
        .arg("make")
        .arg("install")
        .current_dir(&repo_path);
    run_command(cmd_install, "Failed to install Neovim")?;

    println!(
        "Build and install process finished. The repository is kept at {:?}",
        repo_path
    );

    Ok(())
}

/// Create symbolic link to "~/.config/nvim"
fn create_symlink() -> Result<()> {
    let dotfiles_path = env::current_dir().context("Failed to get current directory")?;
    let source_path = dotfiles_path.join(".config/nvim");

    let home_path = home::home_dir().context("Failed to get home directory")?;
    let destination_path = home_path.join(".config/nvim");

    println!("- Source: {}", source_path.display());
    println!("- Destination: {}", destination_path.display());

    if let Some(parent) = destination_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directory: {}", parent.display())
            })?;
        }
    }

    if destination_path.exists() || destination_path.symlink_metadata().is_ok() {
        if let Ok(existing_target) = fs::read_link(&destination_path) {
            if existing_target == source_path {
                println!("symbolic link already exists and is correct. Skipping.");
                return Ok(());
            }
        }
        println!(
            "Destination path '{}' already exists. Please back it up or remove it first.",
            destination_path.display()
        );
        return Ok(());
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&source_path, &destination_path).with_context(|| {
            format!(
                "Failed to create symbolic link from {} to {}",
                source_path.display(),
                destination_path.display()
            )
        })?;
        println!("Successfully created symbolic link.");
    }
    // #[cfg(not(unix))]
    // {
    //     println!("Symbolic link creation is supported on Unix-like(WSL, Linux, macOS) systems.");
    // }

    Ok(())
}

/// Execute command, then if failed, adding error message
fn run_command(mut command: Command, error_message: &str) -> Result<()> {
    let status = command
        .status()
        .context(format!("Failed to execute command: {:?}", command))?;
    if !status.success() {
        anyhow::bail!("{}: Command exited with non-zero status.", error_message);
    }
    Ok(())
}

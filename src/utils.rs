use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::process::Command;

/// Create symbolic link to "~/.config/nvim"
pub fn create_symlink(source: &str, destination: &str) -> Result<()> {
    let dotfiles_path = env::current_dir().context("Failed to get current directory")?;
    let source_path = dotfiles_path.join(source);

    let home_path = home::home_dir().context("Failed to get home directory")?;
    let destination_path = home_path.join(destination);

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
pub fn run_command(mut command: Command, error_message: &str) -> Result<()> {
    let status = command
        .status()
        .context(format!("Failed to execute command: {:?}", command))?;
    if !status.success() {
        anyhow::bail!("{}: Command exited with non-zero status.", error_message);
    }
    Ok(())
}

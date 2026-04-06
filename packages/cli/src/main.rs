mod alacritty;
mod claude;
mod codex;
mod common;
mod gemini;
mod ghostty;
mod neovim;
mod tmux;
mod utils;
mod wezterm;
mod zellij;
mod zsh;

use std::path::PathBuf;

use crate::common::Distro;
use clap::{Parser, Subcommand};

/// Command line tool for dotfiles setup
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Specify the Linux distribution",
        default_value = "ubuntu"
    )]
    distro: Distro,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Neovimのセットアップ（指定タグでビルド・インストール）
    Neovim {
        /// ビルドするGitタグ（例: v0.12.0）
        #[arg(long)]
        tag: String,
    },
    /// Neovimを指定タグにアップデート
    NeovimUpdate {
        /// アップデート先のGitタグ（例: v0.12.0）
        #[arg(long)]
        tag: String,
    },
    /// Build and deploy the nvim-config Rust library
    BuildNvimConfig,
    /// Setup for Zsh
    Zsh,
    /// Setup for Wezterm
    Wezterm,
    /// Setup for Alacritty
    Alacritty,
    /// Setup for Ghostty
    Ghostty,
    /// Setup for Zellij
    Zellij,
    /// Setup for tmux
    Tmux,
    /// Setup for Claude Code
    Claude,
    /// Setup for Codex CLI
    Codex,
    /// Setup for Gemini CLI
    Gemini,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Neovim { tag } => {
            println!(
                "🚀 Neovim ({}) のセットアップを開始します ({:?}) ...",
                tag, cli.distro
            );
            neovim::setup(&cli.distro, &tag)?;
            println!("\n✅ Neovimのセットアップが完了しました!");
        }
        Commands::NeovimUpdate { tag } => {
            println!(
                "🔄 Neovimをタグ {} にアップデートします ({:?}) ...",
                tag, cli.distro
            );
            neovim::update(&cli.distro, &tag)?;
            println!("\n✅ Neovimのアップデートが完了しました!");
        }
        Commands::BuildNvimConfig => {
            println!("🦀 Building nvim-config library...");
            let status = std::process::Command::new("cargo")
                .arg("build")
                .arg("-p")
                .arg("nvim-config")
                .arg("--release")
                .status()?;

            if !status.success() {
                anyhow::bail!("Failed to build nvim-config library");
            }
            println!("✅ Build successful!");
            println!("🚚 Copying library to Neovim config directory...");

            let lib_name = "nvim_config";
            let extension = if cfg!(target_os = "macos") {
                "dylib"
            } else {
                "so"
            };

            let source_path = PathBuf::from(format!("./target/release/lib{lib_name}.{extension}"));

            let dest_dir = home::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
                .join(".config/nvim/lua");

            std::fs::create_dir_all(&dest_dir)?;

            let dest_path = dest_dir.join(format!("{lib_name}.{extension}"));

            std::fs::copy(&source_path, &dest_path)?;

            println!("✅ Library copied successfully to {}", dest_path.display());
        }
        Commands::Zsh => {
            println!("🚀 Starting Zsh setup for {:?} ...", cli.distro);
            zsh::setup(&cli.distro)?;
            println!("\n✅ Zsh setup completed successfully!");
        }
        Commands::Wezterm => {
            println!("🚀 Starting Wezterm setup for {:?} ...", cli.distro);
            wezterm::setup(&cli.distro)?;
            println!("\n✅ Wezterm setup completed successfully!");
        }
        Commands::Alacritty => {
            println!("🚀 Starting Alacritty setup for {:?} ...", cli.distro);
            alacritty::setup(&cli.distro)?;
            println!("\n✅ Alacritty setup completed successfully!");
        }
        Commands::Ghostty => {
            println!("🚀 Starting Ghostty setup for {:?} ...", cli.distro);
            ghostty::setup(&cli.distro)?;
            println!("\n✅ Ghostty setup completed successfully!");
        }
        Commands::Zellij => {
            println!("🚀 Starting Zellij setup for {:?} ...", cli.distro);
            zellij::setup(&cli.distro)?;
            println!("\n✅ Zellij setup completed successfully!");
        }
        Commands::Tmux => {
            println!("🚀 Starting tmux setup for {:?} ...", cli.distro);
            tmux::setup(&cli.distro)?;
            println!("\n✅ tmux setup completed successfully!");
        }
        Commands::Claude => {
            claude::setup()?;
        }
        Commands::Codex => {
            codex::setup()?;
        }
        Commands::Gemini => {
            gemini::setup()?;
        }
    }
    Ok(())
}

mod common;
mod neovim;
mod utils;
mod wezterm;
mod zsh;

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
    /// Setup for Neovim
    Neovim,
    /// Setup for Zsh
    Zsh,
    /// Setup for Wezterm
    Wezterm,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Neovim => {
            println!("🚀 Starting Neovim setup for {:?} ...", cli.distro);
            neovim::setup(&cli.distro)?;
            println!("\n✅ Neovim setup completed successfully!");
        }
        Commands::Zsh => {
            println!("🚀 Starting Zsh setup for {:?} ...", cli.distro);
            zsh::setup(&cli.distro)?;
            println!("\n✅ Zsh setup completed successfully!");
        }
        Commands::Wezterm => {
            println!("🚀 Starting Wezterm setup for {:?} ...", cli.distro);
            wezterm::setup(&cli.distro)?;
            println!("\n✅ Zsh setup completed successfully!");
        }
    }
    Ok(())
}

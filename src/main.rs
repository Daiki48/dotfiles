use clap::{Parser, Subcommand};
mod neovim;

/// Command line tool for dotfiles setup
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Setup for Neovim
    Neovim,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Neovim => {
            println!("🚀 Starting Neovim setup...");
            neovim::setup()?;
            println!("\n✅ Neovim setup completed successfully!");
        }
    }
    Ok(())
}

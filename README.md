# My Setup

## Font

- [JetBrains Mono](https://www.nerdfonts.com/font-downloads) for Programming
- [BIZ UDPGothic](https://fonts.google.com/specimen/BIZ+UDPGothic) for Japanese

## For Windows setup

```sh
.\setup_windows.ps1
```

## Setup script

### Prerequisites

#### Ubuntu (via WSL)

```sh
wsl.exe --install --no-distribution
```

Reboot PC.

```sh
wsl --install
```

Install [Rust](https://www.rust-lang.org/tools/install):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

```sh
sudo apt install -y build-essential x11-apps wl-clipboard
```

### CLI Reference

This repository ships a Rust CLI under `packages/cli` that handles installation,
version checks, and configuration symlinking for every tool listed below.

```sh
cargo run -- [--distro <ubuntu|fedora>] <command> [command options]
```

- `--distro` is a global flag and defaults to `ubuntu`. Pass `fedora` on Fedora-based systems.
- A few commands ignore `--distro` because they install via curl or npm regardless of the host distro.
- Place `--distro` either before or after the subcommand — both are accepted by clap.

#### Commands

| Command | Description | Honors `--distro` |
|---------|-------------|:-----------------:|
| `zsh` | Install zsh (apt on Ubuntu / dnf on Fedora), symlink `.zshrc`, `.zsh/`, `.zprofile`, `.profile`, then run `chsh -s $(which zsh)` to change the default login shell. | yes |
| `neovim --tag <tag>` | Build Neovim from source at the given git tag (e.g. `v0.12.2`) and install it via `sudo make install`. Also symlinks `~/.config/nvim`. If Neovim is already installed, only the symlink step runs. | yes |
| `neovim-update --tag <tag>` | Update an existing Neovim install to the given tag. Runs `git fetch --depth 1` for the tag, `git checkout`, `make distclean`, `make CMAKE_BUILD_TYPE=Release`, then `sudo make install`. The tag is verified against the remote before any work begins. | yes |
| `build-nvim-config` | Build the `nvim-config` Rust library with `cargo build --release -p nvim-config` and copy `libnvim_config.so` (Linux) / `libnvim_config.dylib` (macOS) into `~/.config/nvim/lua/`. The file is **copied**, not symlinked. | no |
| `wezterm` | Install WezTerm. Ubuntu: `sudo apt update` then `sudo apt install -y wezterm`. Fedora: `dnf install` from the official GitHub release rpm. Symlinks `~/.config/wezterm`. | yes |
| `alacritty` | Install Alacritty via apt/dnf and symlink `~/.config/alacritty`. | yes |
| `ghostty` | Install Ghostty. Ubuntu: community installer at `mkasberg/ghostty-ubuntu`. Fedora: `dnf install ghostty`. Symlinks `~/.config/ghostty`. | yes |
| `zellij` | Install Zellij. Ubuntu: `cargo install zellij`. Fedora: `dnf install zellij`. Symlinks `~/.config/zellij`. | yes |
| `tmux` | Install tmux via apt/dnf, clone [TPM](https://github.com/tmux-plugins/tpm) (Tmux Plugin Manager) into `~/.config/tmux/plugins/tpm`, and symlink `~/.config/tmux/tmux.conf`. After setup, press `Ctrl+g` then `I` (capital i) inside tmux to install plugins. | yes |
| `claude` | Install Claude Code via the official installer (`curl -fsSL https://claude.ai/install.sh \| bash`). Symlinks `CLAUDE.md`, `settings.json`, `settings.local.json`, `skills/`, and `agents/` under `~/.claude/`. | no |
| `codex` | Install Codex CLI via `npm install -g @openai/codex`. Symlinks `~/.codex/AGENTS.md` and copies `config.toml` (only if it does not already exist, so local edits are preserved). | no |
| `gemini` | Install Gemini CLI via `npm install -g @google/gemini-cli` and symlink `~/.gemini/settings.json`. Requires `GEMINI_API_KEY` exported in your shell. | no |

#### Examples

##### Ubuntu

```sh
cargo run -- zsh
cargo run -- neovim --tag v0.12.2
cargo run -- neovim-update --tag v0.12.2
cargo run -- build-nvim-config
cargo run -- tmux
cargo run -- claude
cargo run -- codex
cargo run -- gemini
```

##### Fedora

```sh
cargo run -- --distro fedora zsh
cargo run -- --distro fedora neovim --tag v0.12.2
cargo run -- --distro fedora neovim-update --tag v0.12.2
cargo run -- --distro fedora wezterm
cargo run -- --distro fedora tmux
```

After running the `zsh` command, restart your terminal and verify the shell:

```sh
echo $SHELL
```

It should print `/usr/bin/zsh` (or `/bin/zsh`).

### .skk

[CorvusSKK](https://nathancorvussolis.github.io/)

```sh
winget install -h corvusskk -s winget
```

Dictionary.

```shell
git clone https://github.com/Daiki48/skk.git
```

Config for CorvusSKK.

[CorvusSKK config](https://github.com/Daiki48/dotfiles/blob/main/docs/corvusskk.ja.md)

## Setup Documentation

- [skk](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-skk.md)
- [skkeleton](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-skkeleton.md)
- [docker](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-docker.md)

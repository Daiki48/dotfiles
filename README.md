# My Setup

## Font

- [JetBrains Mono](https://www.nerdfonts.com/font-downloads) for Programming
- [BIZ UDPGothic](https://fonts.google.com/specimen/BIZ+UDPGothic) for Japanese

## For Windows setup

```sh
.\setup_windows.ps1
```

## Setup script

### For Ubuntu

```sh
wsl.exe --install --no-distribution
```

Reboot PC.

```sh
wsl --install
```

The first step is to install [Rust](https://www.rust-lang.org/tools/install) .

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

```sh
sudo apt install -y build-essential x11-apps wl-clipboard
```

### 1. zsh

Required [Rust](https://www.rust-lang.org/) .

```sh
cargo run -- zsh
```

For Fedora.

```sh
cargo run -- --distro fedora zsh
```

Restart terminal.  
Checking current Shell.

```sh
echo $SHELL
```

If it prints `usr/bin/zsh` , it success.

### 2. Neovim

```sh
cargo run -- neovim
```

For Fedora.

```sh
cargo run -- --distro fedora neovim
```

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

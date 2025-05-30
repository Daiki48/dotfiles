# My Setup

## Font

- [JetBrains Mono](https://www.jetbrains.com/lp/mono/) for Programming
- [BIZ UDPGothic](https://fonts.google.com/specimen/BIZ+UDPGothic) for Japanese

## For Windows setup

```sh
.\setup_windows.ps1
```

## For WSL2 (Ubuntu) setup

### 1. zsh

```sh
cargo run -- zsh
```

Restart terminal.  
Checking current Shell.

```sh
echo $SHELL
```

If it prints `usr/bin/zsh` , it success.

### 2. Neovim

Required [Rust](https://www.rust-lang.org/) .

```sh
cargo run -- neovim
```

### .skk

```shell
git clone https://github.com/Daiki48/skk.git
```

## Setup Documentation

- [skk](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-skk.md)
- [skkeleton](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-skkeleton.md)
- [docker](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-docker.md)

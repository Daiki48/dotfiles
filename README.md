# My Setup

## Font

- [JetBrains Mono](https://www.jetbrains.com/lp/mono/)

## Symbolic Link

### nvim

```shell
ln -s ~/dotfiles/.config/nvim ~/.config/
```

or powershell

```shell
New-Item -ItemType SymbolicLink -Path $env:USERPROFILE\AppData\Local\nvim -Target $env:USERPROFILE\dotfiles\.config\nvim
```

### wezterm

```shell
ln -s ~/dotfiles/.config/nvim ~/.config/ 
```

### .zshrc

Backup and create existing `.zshrc` .

```shell
mv .zshrc .zshrc_backup
ln -s ~/dotfiles/.zshrc ~/
```

### .zsh

```shell
ln -s ~/dotfiles/.zsh ~/.zsh
```

### .skk

```shell
git clone https://github.com/Daiki48/skk.git
```

## Setup Documentation

- [skk](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-skk.md)
- [skkeleton](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-skkeleton.md)
- [docker](https://github.com/Daiki48/dotfiles/blob/main/docs/setup-docker.md)

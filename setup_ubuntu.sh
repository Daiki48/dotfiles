#!/bin/zsh

echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo ""
echo "Starting setup for WSL2 (zsh integrated, Neovim (snap), Deno, Rust, Node.js)"
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo ""
echo ""
echo "Checking zsh installation..."
echo ""
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"

# zsh インストールチェック (インストールされていなければエラーメッセージを表示して終了)
if ! command -v zsh >/dev/null 2>&1; then
  echo "zsh is not installed."
  echo ""
  echo "Please run './setup_zsh.sh' first to install and setup zsh, and then run this script again in a zsh shell."
  echo ""
  echo "Setup aborted."
  exit 1
fi

zshVersion=$(zsh --version)
echo "zsh is already installed. Version: ${zshVersion}"
echo ""
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo ""
echo "Continuing setup for Neovim (snap), Deno, Rust, and Node.js (via nvm)."
echo ""
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"


# Snap インストールとバージョン確認
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo "Checking snap installation..."
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
if ! command -v snap >/dev/null 2>&1; then
  echo "snap is not installed. Installing snap..."
  sudo apt update
  sudo apt install -U -y snapd # snap をインストール (未インストール時のみ)
  if ! command -v snap >/dev/null 2>&1; then
    echo "Failed to install snap. Please check the installation manually."
    exit 1
  fi
  echo "snap installed successfully."
else
	snapVersion=$(snap --version)
  echo "snap is already installed. Version: ${snapVersion}"
fi


# Neovim (snap) インストールとバージョン確認
source scripts/setup_neovim.sh

echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo ""
echo "Checking symbolic link for Neovim config in WSL2 config directory"
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
nvimConfigPath="$HOME/.config/nvim"

if [ -L "${nvimConfigPath}" ]; then
  target=$(readlink "${nvimConfigPath}")
  echo ".config/nvim in WSL2 is a symbolic link pointing to: ${target}"
elif [ -d "${nvimConfigPath}" ]; then
  echo ".config/nvim in WSL2 is not a symbolic link. Not symbolic"
else
  echo ".config/nvim in WSL2 does not exist. Not symbolic"
fi

dotfilesPath=$(pwd)
sourcePath="${dotfilesPath}/.config/nvim"
destinationPath="$HOME/.config/nvim"

if [ ! -e "${destinationPath}" ]; then
  mkdir -p "$(dirname "${destinationPath}")"
  ln -sf "${sourcePath}" "${destinationPath}"
  if [ -L "${destinationPath}" ]; then
    echo "Created symbolic link for nvim in WSL2 .config/nvim successfully."
  else
    echo "Failed to create symbolic link for nvim in WSL2 .config/nvim."
  fi
else
  echo ".config/nvim in WSL2 already exists. Skipping symbolic link creation."
fi


echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo "Setup Partially Successful!" # 部分的に成功に変更
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo ""
echo "Installed:"
echo "- zsh (Version: ${zshVersion})"
echo "- snap (Version: ${snapVersion})"
echo "- Neovim (snap) (Version: ${nvimVersion})"
echo ""
echo "Neovim config symbolic link created from:"
echo "- Source:       ${sourcePath}"
echo "- Destination:  ${destinationPath}"
echo ""
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo "Next Steps (IMPORTANT):" # 重要であることを強調
echo ""
echo "1. Run './setup_zsh.sh' to install and setup zsh dotfiles (if you haven't already)." # setup_zsh.sh 実行を促す
echo ""
echo "2. Change your default shell to zsh by running the following command in the terminal:"
echo "   chsh -s $(which zsh)"
echo ""
echo "3. Restart your terminal (close and reopen)."
echo ""
echo "4. Verify that your default shell has been changed to zsh by running:"
echo "   echo \$SHELL"
echo "   It should output: /usr/bin/zsh"
echo ""
echo "5. After changing to zsh, re-run './setup_ubuntu.sh' to complete the setup." # setup_ubuntu.sh 再実行を促す
echo ""
echo "6. Verify Deno installation and version by restarting your terminal or opening a new shell and running: deno --version" # Deno バージョン確認手順を追加
echo "7. Verify Node.js installation and version by restarting your terminal or opening a new shell and running: node --version" # Node.js バージョン確認手順を追加
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo ""
echo ""
echo ""
echo ""
echo ""
echo ""
echo ""
echo "See you! (Ubuntu Setup)" # メッセージを修正

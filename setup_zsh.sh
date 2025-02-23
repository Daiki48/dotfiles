#!/bin/bash

echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo ""
echo "Starting setup for zsh and dotfiles (zshrc, .zsh)"
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo ""
echo ""
echo "Checking and installing zsh, and creating symbolic links for dotfiles."
echo ""
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"

# zsh インストールとバージョン確認
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo "Checking zsh installation..."
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
if ! command -v zsh >/dev/null 2>&1; then
  echo "zsh is not installed. Installing zsh..."
  sudo apt update # パッケージリストを更新
  sudo apt install -y zsh  # zshをインストール (Ubuntu/Debian 前提)
  if ! command -v zsh >/dev/null 2>&1; then
    echo "Failed to install zsh. Please check the installation manually."
    exit 1
  fi
  echo "zsh installed successfully."
else
  echo "zsh is already installed."
fi
zshVersion=$(zsh --version)
echo "zsh Version: ${zshVersion}"


echo ""
echo ""
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo ""
echo "Creating symbolic links for .zshrc and .zsh from dotfiles"
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"

dotfilesPath=$(pwd) # スクリプトが実行されたディレクトリをdotfilesのルートディレクトリと仮定

# .zshrc のシンボリックリンク作成
sourceZshrcPath="${dotfilesPath}/.zshrc"
destinationZshrcPath="$HOME/.zshrc"

echo ""
echo "Checking symbolic link for .zshrc"
if [ -L "${destinationZshrcPath}" ]; then
  target=$(readlink "${destinationZshrcPath}")
  echo ".zshrc in ~ is a symbolic link pointing to: ${target}"
elif [ -e "${destinationZshrcPath}" ]; then
  echo ".zshrc in ~ already exists but is not a symbolic link. Skipping .zshrc symbolic link creation."
else
  if [ -e "${sourceZshrcPath}" ]; then
    ln -sf "${sourceZshrcPath}" "${destinationZshrcPath}"
    if [ -L "${destinationZshrcPath}" ]; then
      echo "Created symbolic link for .zshrc in ~ successfully."
    else
      echo "Failed to create symbolic link for .zshrc in ~."
    fi
  else
    echo "Source .zshrc file not found in dotfiles repository: ${sourceZshrcPath}. Skipping .zshrc symbolic link creation."
  fi
fi


# .zsh ディレクトリのシンボリックリンク作成
sourceZshDirPath="${dotfilesPath}/.zsh"
destinationZshDirPath="$HOME/.zsh"

echo ""
echo "Checking symbolic link for .zsh directory"
if [ -L "${destinationZshDirPath}" ]; then
  target=$(readlink "${destinationZshDirPath}")
  echo ".zsh directory in ~ is a symbolic link pointing to: ${target}"
elif [ -e "${destinationZshDirPath}" ]; then
  echo ".zsh directory in ~ already exists but is not a symbolic link. Skipping .zsh directory symbolic link creation."
else
  if [ -d "${sourceZshDirPath}" ]; then
    mkdir -p "$(dirname "${destinationZshDirPath}")" # 念のためdestinationディレクトリの親ディレクトリを作成
    ln -sf "${sourceZshDirPath}" "${destinationZshDirPath}"
    if [ -L "${destinationZshDirPath}" ]; then
      echo "Created symbolic link for .zsh directory in ~ successfully."
    else
      echo "Failed to create symbolic link for .zsh directory in ~."
    fi
  else
    echo "Source .zsh directory not found in dotfiles repository: ${sourceZshDirPath}. Skipping .zsh directory symbolic link creation."
  fi
fi


echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo "Setup Successful!"
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo ""
echo "Installed:"
echo "- zsh (Version: ${zshVersion})"
echo ""
echo "Created symbolic links (if applicable) from dotfiles:"
echo "- .zshrc: Source: ${sourceZshrcPath}, Destination: ${destinationZshrcPath}"
echo "- .zsh  : Source: ${sourceZshDirPath},  Destination: ${destinationZshDirPath}"
echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo "Next Steps:"
echo ""
echo "1. Change your default shell to zsh by running the following command in the terminal:"
echo "   chsh -s $(which zsh)"
echo ""
echo "2. Restart your terminal (close and reopen)."
echo ""
echo "3. Verify that your default shell has been changed to zsh by running:"
echo "   echo \$SHELL"
echo "   It should output: /usr/bin/zsh"
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo ""
echo ""
echo ""
echo ""
echo ""
echo ""
echo ""
echo "See you! (zsh)"

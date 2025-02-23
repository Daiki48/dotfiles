#!/bin/zsh

echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo "Checking Neovim (snap) installation..."
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"

if [[ ":$PATH:" != *":/snap/bin:"* ]]; then
    export PATH="$PATH:/snap/bin"
fi

if ! command -v nvim >/dev/null 2>&1; then
    echo "Neovim is not installed or not in PATH. Installing Neovim (snap)..."
    sudo snap install nvim --classic
    if ! command -v nvim >/dev/null 2>&1; then
        echo "Failed to install Neovim (snap). Please check the installation manually."
        exit 1
    fi
    echo "Neovim (snap) installed successfully."
fi

nvimVersion=$(nvim --version | head -n 1)
echo "Neovim (snap) is already installed via snap. Version: ${nvimVersion}"

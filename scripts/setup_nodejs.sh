#!/bin/zsh

echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo "Checking Node.js (nvm) installation..."
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
if ! command -v nvm >/dev/null 2>&1; then
  echo "nvm is not installed. Installing nvm and Node.js (latest LTS)..."
  NVM_INSTALL_RESULT=$(curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.7/install.sh) # 結果を変数に格納
  bash -c "$NVM_INSTALL_RESULT" # スクリプトを実行 (bash経由で実行)

# nvm の PATH 設定を .zshrc に追記 (存在しなければ追記)
  if ! grep -q 'export NVM_DIR=' ~/.zshrc; then
    echo 'export NVM_DIR="$HOME/.nvm"' >> ~/.zshrc
    echo '[ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"' >> ~/.zshrc
    echo '[ -s "$NVM_DIR/bash_completion" ] && \. "$NVM_DIR/bash_completion"' >> ~/.zshrc
    echo "nvm PATH configuration to be added to ~/.zshrc:" # 出力メッセージ変更
    echo "export NVM_DIR=\"\$HOME/.nvm\"" # PATH設定内容を出力
    echo "[ -s \"\$NVM_DIR/nvm.sh\" ] && . \"\$NVM_DIR/nvm.sh\"" # PATH設定内容を出力
    echo "[ -s \"\$NVM_DIR/bash_completion\" ] && . \"\$NVM_DIR/bash_completion\"" # PATH設定内容を出力
    echo ""
    echo "Please verify Node.js installation and version by restarting your terminal or opening a new shell and running: node --version"
  else
    echo "nvm PATH configuration already exists in ~/.zshrc"
    echo "Please verify Node.js installation and version by restarting your terminal or opening a new shell and running: node --version"
  fi

  export NVM_DIR="$HOME/.nvm" # NVM_DIR を設定 (追記)
  [ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh" # nvm.sh を source (追記)
  [ -s "$NVM_DIR/bash_completion" ] && \. "$NVM_DIR/bash_completion" # bash_completion を source (追記)


  nvm install --lts # インストールは行う (PATH設定は.zshrcで行う)
  nvm alias default lts # デフォルト設定
  if ! command -v nvm >/dev/null 2>&1 || ! command -v node >/dev/null 2>&1; then # nvm 自体のインストール失敗チェックを行う
    echo "Failed to install nvm and Node.js (nvm). Please check the installation manually and ~/.zshrc."
    exit 1
  else
    echo "nvm and Node.js installation started and PATH configured in ~/.zshrc. Node.js installation may continue in background."
  fi
 
else
  echo "nvm is already installed. Installing latest LTS Node.js..."
  export NVM_DIR="$HOME/.nvm"
  [ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"
  [ -s "$NVM_DIR/bash_completion" ] && \. "$NVM_DIR/bash_completion"
  nvm install --lts
  nvm alias default lts
  nodeVersion=$(node --version) # インストール済みの場合のみバージョン確認
  echo "Installed latest LTS Node.js. Version: ${nodeVersion}"
fi

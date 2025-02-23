#!/bin/zsh

echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo "Checking Deno installation..."
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
if ! command -v deno >/dev/null 2>&1; then
  echo "Deno is not installed. Installing Deno..."
  DENO_INSTALL_RESULT=$(curl -fsSL https://deno.land/install.sh) # 結果を変数に格納
  eval "$DENO_INSTALL_RESULT" # スクリプトを実行

  # Deno の PATH 設定を .zshrc に追記 (存在しなければ追記)
  if ! grep -q 'export DENO_INSTALL="' ~/.zshrc; then
    echo 'export DENO_INSTALL="$HOME/.deno"' >> ~/.zshrc
    echo 'export PATH="$DENO_INSTALL/bin:$PATH"' >> ~/.zshrc
    echo "Deno PATH configuration to be added to ~/.zshrc:" # 出力メッセージ変更
    echo "export DENO_INSTALL=\"\$HOME/.deno\"" # PATH設定内容を出力
    echo "export PATH=\"\$DENO_INSTALL/bin:\$PATH\"" # PATH設定内容を出力
    echo ""
    echo "Please verify Deno installation and version by restarting your terminal or opening a new shell and running: deno --version"
  else
    echo "Deno PATH configuration already exists in ~/.zshrc"
    echo "Please verify Deno installation and version by restarting your terminal or opening a new shell and running: deno --version"
  fi

else
  echo "Deno is already installed."
  denoVersion=$(deno --version) # インストール済みの場合のみバージョン確認
  echo "Deno Version: ${denoVersion}"
fi

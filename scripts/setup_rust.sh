#!/bin/zsh

echo ""
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
echo "Checking Rust installation..."
echo "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
if ! command -v rustup >/dev/null 2>&1; then
  echo "Rust is not installed. Installing Rust..."
  curl --proto '=https' -tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  if ! command -v rustup >/dev/null 2>&1; then
    echo "Failed to install Rust (rustup). Please check the installation manually."
    exit 1
  fi
  echo "Rust (rustup) installed successfully."
  rustVersion=$(rustup --version)
  echo "Rust Version: ${rustVersion}"

  # Rust (Cargo) の PATH 設定を .zshrc に追記 (存在しなければ追記)
  if ! grep -q 'export PATH="\$HOME/.cargo/bin:\$PATH"' ~/.zshrc; then # 追記済みのPATHをgrepで確認 (完全一致)
    echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc
    echo "Rust (Cargo) PATH configuration to be added to ~/.zshrc:" # 出力メッセージ変更
    echo "export PATH=\"\$HOME/.cargo/bin:\$PATH\"" # PATH設定内容を出力
    echo ""
    echo "Please verify Rust installation and version by restarting your terminal or opening a new shell and running: rustc --version" # rustc でバージョン確認
  else
    echo "Rust (Cargo) PATH configuration already exists in ~/.zshrc"
    echo "Please verify Rust installation and version by restarting your terminal or opening a new shell and running: rustc --version" # rustc でバージョン確認
  fi


else
  rustVersion=$(rustup --version)
  echo "Rust (rustup) is already installed. Version: ${rustVersion}"
  echo "Rust (Cargo) PATH may already be configured in ~/.zshrc." # PATH設定済みである可能性を示唆
  echo "Please verify Rust installation and version by running: rustc --version" # rustc でバージョン確認を促す
fi

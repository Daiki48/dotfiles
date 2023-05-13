echo "Setup Start!!"
sudo apt update
apt-get update
apt install -y curl git ripgrep unzip vim wget
apt install -y locales locale-gen ja_JP.UTF-8
apt install build-essential # C compiler

# Check arg 
if [ "$1" = "-y" ]; then
  non_interactive=true
else
  non_interactive=false
fi

zsh_installed=false

echo " ############# zsh setup ############# "
if [ "$non_interactive" = true ]; then
  Answer="y"
else
  printf "Do you want to install zsh? (Y/n) [y]: "
  read Answer < /dev/tty
  Answer=${Answer:-y}
fi

case ${Answer} in
  y|Y) 

    echo "Install zsh..."

    apt install zsh -y

    zsh_installed=true
    echo "Successfully installed!!"

    if [ "$non_interactive" = true ]; then
      Answer="y"
    else
      printf "Do you want to set zsh as the default shell? (Y/n) [y]: "
      read Answer < /dev/tty
      Answer=${Answer:-y}
    fi

    case ${Answer} in
      y|Y) 

        echo "Setting zsh as the default shell..."
        chsh -s $(which zsh)
        ln -s ~/dotfiles/.zshrc ~/
        echo "Successfully set!!" ;;

      n|N)

        echo "Skipped setting." ;;

    esac ;;

  n|N)
    echo "Skipped installation." ;;

esac


echo " ############# zsh setup finish! ############# "

echo " ############# neovim setup ############# "
if [ "$non_interactive" = true ]; then
  Answer="y"
else
  printf "Do you want to install neovim? (Y/n) [y]: "
  read Answer < /dev/tty
  Answer=${Answer:-y}
fi

case ${Answer} in
  y|Y) 

    echo "Install neovim..."

    wget https://github.com/neovim/neovim/releases/download/v0.8.2/nvim-linux64.deb
    apt install ./nvim-linux64.deb
    rm ./nvim-linux64.deb


    wget https://raw.githubusercontent.com/Shougo/dein-installer.vim/master/installer.sh
    sh installer.sh '~/.cache/dein' --use-neovim-config
    rm installer.sh

    curl -fsSL https://deb.nodesource.com/setup_18.x | bash
    apt install -y nodejs
    npm install -g vim-language-server

    rm ~/.bashrc
    rm ~/.config/nvim/init.vim
    ln -s ~/dotfiles/.config/nvim ~/.config/

    echo "Successfully installed!!" ;;
  n|N)
    echo "Skipped installation." ;;

esac

echo " ############# neovim setup finish! ############# "

echo " ############# deno setup ############# "
if [ "$non_interactive" = true ]; then
  Answer="y"
else
  printf "Do you want to install deno? (Y/n) [y]: "
  read Answer < /dev/tty
  Answer=${Answer:-y}
fi

case ${Answer} in
  y|Y) 

    echo "Install deno..."
    curl -fsSL https://deno.land/install.sh | DENO_INSTALL=/usr/local sh

    echo "Successfully installed!!" ;;
  n|N)
    echo "Skipped installation." ;;

esac

echo " ############# deno setup finish! ############# "

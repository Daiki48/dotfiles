#!/bin/sh

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

    sudo apt install zsh -y

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
    wget https://github.com/neovim/neovim/releases/download/v0.9.0/nvim.appimage
    chmod a+x nvim.appimage
    sudo apt-get install fuse -y
    sudo mv nvim.appimage /usr/local/bin/nvim

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
    sudo apt-get install unzip -y
    curl -fsSL https://deno.land/x/install/install.sh | sh

    if [ "$zsh_installed" = true ]; then
      echo 'export DENO_INSTALL="/home/$USER/.deno"' >> ~/.zshrc
      echo 'export PATH="$DENO_INSTALL/bin:$PATH"' >> ~/.zshrc

      source ~/.zshrc
    else
      echo 'export DENO_INSTALL="/home/$USER/.deno"' >> ~/.bashrc
      echo 'export PATH="$DENO_INSTALL/bin:$PATH"' >> ~/.bashrc

      source ~/.bashrc
    fi

    echo "Successfully installed!!" ;;
  n|N)
    echo "Skipped installation." ;;

esac

echo " ############# deno setup finish! ############# "

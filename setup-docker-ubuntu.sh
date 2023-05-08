#!/bin/sh

echo "apt update & install wget curl... "
apt-get update && apt-get install -y wget curl

# Check arg 
if [ "$1" = "-y" ]; then
  non_interactive=true
else
  non_interactive=false
fi

clear

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
    apt-get install fuse -y
    mv nvim.appimage /usr/local/bin/nvim

    echo "Successfully installed!!" ;;
  n|N)
    echo "Skipped installation." ;;

esac

echo " ############# neovim setup finish! ############# "

clear

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
    apt install unzip -y
    curl -fsSL https://deno.land/x/install/install.sh | sh

    
    echo 'export DENO_INSTALL="/home/$USER/.deno"' >> ~/.bashrc
    echo 'export PATH="$DENO_INSTALL/bin:$PATH"' >> ~/.bashrc

    echo "Successfully installed!!" ;;
  n|N)
    echo "Skipped installation." ;;

esac

echo " ############# deno setup finish! ############# "


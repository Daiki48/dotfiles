# This script file is using at my https://github.com/Daiki48/Dockerfiles.git.


# Check arg 
if [ "$1" = "-y" ]; then
  non_interactive=true
else
  non_interactive=false
fi

echo " ---------------------------------------- "
echo " ############# neovim setup ############# "
echo " ---------------------------------------- "
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

    echo "Successfully installed!!" ;;
  n|N)
    echo "Skipped installation." ;;

esac

echo " ------------------------------------------------ "
echo " ############# neovim setup finish! ############# "
echo " ------------------------------------------------ "
echo ""
echo " -------------------------------------- "
echo " ############# deno setup ############# "
echo " -------------------------------------- "
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

echo " ---------------------------------------------- "
echo " ############# deno setup finish! ############# "
echo " ---------------------------------------------- "
echo ""
echo " -------------------------------------- "
echo " ############# node setup ############# "
echo " -------------------------------------- "
if [ "$non_interactive" = true ]; then
  Answer="y"
else
  printf "Do you want to install node? (Y/n) [y]: "
  read Answer < /dev/tty
  Answer=${Answer:-y}
fi

case ${Answer} in
  y|Y) 

    echo "Install node..."

    curl -fsSL https://deb.nodesource.com/setup_18.x | bash - &&\
    apt install -y nodejs && \
    npm install -g vim-language-server typescript-language-server

    echo "Successfully installed!!" ;;
  n|N)
    echo "Skipped installation." ;;

esac

echo " ---------------------------------------------- "
echo " ############# node setup finish! ############# "
echo " ---------------------------------------------- "
echo ""
echo " ------------------------------------------------- "
echo " ############# Create symbolic links ############# "
echo " ------------------------------------------------- "
if [ "$non_interactive" = true ]; then
  Answer="y"
else
  printf "Do you want to create a symbolic link? (Y/n) [y]: "
  read Answer < /dev/tty
  Answer=${Answer:-y}
fi

case ${Answer} in
  y|Y) 

    echo "Create symbolic links..."

    rm ~/.bashrc
    rm ~/.config/nvim/init.vim
    ln -s /dotfiles/.bashrc ~/.bashrc
    ln -s /dotfiles/.config/nvim ~/.config/
#     ln -s /dotfiles/.local/share/dein/toml ~/.local/share/dein/toml
#     ln -s /dotfiles/.local/share/dein/plugins ~/.local/share/dein/plugins

    echo "Successfully created!!" ;;
  n|N)
    echo "Skipped creation." ;;

esac

echo " --------------------------------------------------------- "
echo " ############# Create symbolic links finish! ############# "
echo " --------------------------------------------------------- "



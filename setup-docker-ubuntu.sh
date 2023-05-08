# Check arg 
if [ "$1" = "-y" ]; then
  non_interactive=true
else
  non_interactive=false
fi

echo " -------------------------------------- "
echo " ############# init setup ############# "
echo " -------------------------------------- "

printf "Install curl & wget & git & ripgrep & unzip & vim ..."

apt update
apt-get update
apt install -y curl git ripgrep unzip vim wget

printf "Successfully installed curl & wget & git & ripgrep & unzip & vim !!"

echo " --------------------------------------------- "
echo " ############# init setup finish ############# "
echo " --------------------------------------------- "
echo ""
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


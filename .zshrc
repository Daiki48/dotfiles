# Add deno completions to search path
if [[ ":$FPATH:" != *":/home/daiki48/.zsh/completions:"* ]]; then export FPATH="/home/daiki48/.zsh/completions:$FPATH"; fi
setopt print_eight_bit
setopt no_beep

# git 
# curl -o .git-completion.sh https://raw.githubusercontent.com/git/git/master/contrib/completion/git-completion.bash
# curl -o .git-prompt.sh https://raw.githubusercontent.com/git/git/master/contrib/completion/git-prompt.sh
# source ~/.zsh/git-prompt.sh fpath=(~/.zsh $fpath)
# zstyle ':completion:*:*:git:*' script ~/.zsh/git-completion.zsh
# autoload -Uz compinit && compinit
# autoload -Uz vcs_info setopt prompt_subst
# zstyle ':vcs_info:git:*' check-for-changes true

source ~/.zsh/git-prompt.sh
fpath=(~/.zsh $fpath)
zstyle ':completion:*:*:git:*' script ~/.zsh/git-completion.zsh
autoload -Uz compinit && compinit
autoload -Uz vcs_info
setopt prompt_subst
zstyle ':vcs_info:git:*' check-for-changes true
zstyle ':vcs_info:git:*' stagedstr "%F{magenta}!"
zstyle ':vcs_info:git:*' unstagedstr "%F{yellow}+"
zstyle ':vcs_info:*' formats "%F{cyan}%c%u[%b]%f"
zstyle ':vcs_info:*' actionformats '[%b|%a]'
precmd () { vcs_info }
PROMPT='[%B%F{red}%n:%F{green}%~%f]%F{cyan}$vcs_info_msg_0_%f
%F{yellow}$%f '
# PROMPT='[%B%F{red}%n@%m%f%b:%F{green}%~%f]%F{cyan}$vcs_info_msg_0_%f
# %F{yellow}$%f '

# Default work dir
# cd /mnt/sabrent

# Add write, read permissions
umask 000

# Created by newuser for 5.8.1
# export DENO_INSTALL="/home/daiki/.deno"
# export PATH="$DENO_INSTALL/bin:$PATH"

export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"  # This loads nvm
[ -s "$NVM_DIR/bash_completion" ] && \. "$NVM_DIR/bash_completion"  # This loads nvm bash_completion


# bun completions
# [ -s "/home/daiki/.bun/_bun" ] && source "/home/daiki/.bun/_bun"

# bun
# export BUN_INSTALL="$HOME/.bun"
# export PATH="$BUN_INSTALL/bin:$PATH"
. "/home/daiki48/.deno/env"

export PATH="$PATH:/snap/bin"

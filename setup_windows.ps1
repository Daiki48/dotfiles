Write-Host "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
Write-Host ""
Write-Host "Starting setup for Windows"
Write-Host ""
Write-Host "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
Write-Host ""
Write-Host ""
Write-Host "Let's make sure Scoop and Deno are installed now."
Write-Host ""
Write-Host ""
Write-Host "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
Write-Host ""
Write-Host "Cheking scoop at https://scoop.sh"
Write-Host ""
Write-Host "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
if (Get-Command scoop -ErrorAction SilentlyContinue) {
    $scoopVersion = scoop --version
    Write-Host "Scoop is installed. Version: $scoopVersion Installed" -ForegroundColor Green
} else {
    Write-Host "Scoop is not installed. Not install" -ForegroundColor Red
}

Write-Host ""
Write-Host ""
Write-Host ""
Write-Host "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
Write-Host ""
Write-Host "Cheking deno at https://deno.com"
Write-Host ""
Write-Host "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
if (Get-Command deno  -ErrorAction SilentlyContinue) {
	$denoVersion = deno --version
	Write-Host "Deno is installed. Version: $denoVersion Installed" -ForegroundColor Green
} else {
	Write-Host "Deno is not installed. Not install" -ForegroundColor Red
}

Write-Host ""
Write-Host ""
Write-Host ""
Write-Host "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
Write-Host ""
Write-Host "Cheking symblic for Neovim config"
Write-Host ""
Write-Host "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
$nvimConfigPath = Join-Path $env:LOCALAPPDATA "nvim"
if (Test-Path $nvimConfigPath) {
    $item = Get-Item $nvimConfigPath
    if ($item.LinkType -eq "SymbolicLink") {
        $target = $item | Select-Object -ExpandProperty Target
        Write-Host "nvim in AppData/Local is a symbolic link pointing to: $target" -ForegroundColor Green
    } else {
        Write-Host "nvim in AppData/Local is not a symbolic link. Not symblic" -ForegroundColor Red
    }
} else {
    Write-Host "nvim in AppData/Local does not exist. Not symblic" -ForegroundColor Red
}

$continue = Read-Host "Continue setup? (Y/n)"
if ($continue -ne "Y" -and $continue -ne "y" -and $continue -ne "") {
    Write-Host "Setup aborted."
    return
}

if (!(Get-Command scoop -ErrorAction SilentlyContinue)) {
    Write-Host "Installing Scoop..."
    try {
        Invoke-Expression (New-Object System.Net.WebClient).DownloadString('https://get.scoop.sh')
        if (!(Get-Command scoop -ErrorAction SilentlyContinue)) {
            throw "Scoop installation failed. Please check the installation manually."
        }
        Write-Host "Scoop installed successfully." -ForegroundColor Green
    }
    catch {
        Write-Error "Failed to install Scoop: $($_.Exception.Message)"
        return
    }
}

$dotfilesPath = (Get-Location).Path
$sourcePath = Join-Path $dotfilesPath ".config/nvim"
$destinationPath = Join-Path $env:LOCALAPPDATA "nvim"

if (!(Test-Path -Path $destinationPath)) {
    try {
        New-Item -ItemType SymbolicLink -Path $destinationPath -Target $sourcePath -Force
        Write-Host "Created symbolic link for nvim in AppData/Local successfully." -ForegroundColor Green
    }
    catch {
        Write-Error "Failed to create symbolic link for nvim in AppData/Local: $($_.Exception.Message)"
        return
    }
} else {
    Write-Host "nvim in AppData/Local already exists. Skipping." -ForegroundColor Yellow
}

Write-Host "Successful" -ForegroundColor Green
Write-Host ""
Write-Host "Other items that need to be installed separately..."
Write-Host ""
Write-Host "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
Write-Host "| Rust     | https://www.rust-lang.org/ja/tools/install                |"
Write-Host "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
Write-Host "|CorvusSKK | https://github.com/nathancorvussolis/corvusskk/releases   |"
Write-Host "++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++"
Write-Host ""
Write-Host ""
Write-Host ""
Write-Host ""
Write-Host ""
Write-Host ""
Write-Host ""
Write-Host "See you!"

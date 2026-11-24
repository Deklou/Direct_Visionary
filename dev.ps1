param(
    [ValidateSet('All', 'Build', 'Launch')]
    [string]$Action = 'All'
)

$ErrorActionPreference = 'Stop'
$repo = $PSScriptRoot
$gameId = '01006A800016E000'
$portable = 'C:\Emulateurs\Ryujinx_Modding\portable'
$nro = 'lib_effect_viewer.nro'
$target = Join-Path $portable "sdcard\atmosphere\contents\$gameId\romfs\skyline\plugins\$nro"
$builtNro = Join-Path $repo "plugins\slight_replica\target\aarch64-skyline-switch\release\$nro"
$gui = Join-Path $repo 'target\release\visionary.exe'
$sdDir = Join-Path (Join-Path $HOME 'Desktop') 'Visionary_Test'

function Test-Tool([string]$name) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
        throw "Outil introuvable : $name"
    }
}

function Test-LastExit([string]$label) {
    if ($LASTEXITCODE -ne 0) {
        throw "Echec : $label (code $LASTEXITCODE)"
    }
}

function Start-Visionary {
    $edenDir = Join-Path $env:APPDATA "eden\load\$gameId\Arcropolis"
    if ((Test-Path Env:VISIONARY_EDEN_MOD_DIR) -or (Test-Path -LiteralPath $edenDir)) {
        throw 'Eden detecte : lancement annule.'
    }
    if (-not (Test-Path -LiteralPath $gui -PathType Leaf)) {
        throw "Visionary introuvable : $gui"
    }
    if (-not (Test-Path -LiteralPath $sdDir -PathType Container)) {
        throw "Dossier Visionary introuvable : $sdDir"
    }
    $env:VISIONARY_SD_DIR = $sdDir
    Write-Host 'Lancement de Visionary...'
    & $gui
}

if ($Action -eq 'Launch') {
    Start-Visionary
    return
}

Test-Tool 'git'
Test-Tool 'cargo'
Test-Tool 'wsl.exe'

Push-Location $repo
try {
    $branch = (& git branch --show-current).Trim()
    Test-LastExit 'git branch'
    if ($branch -notin @('dev', 'test/tcp-communication', 'test/visualisation-collisions')) {
        throw "Branche '$branch' : compilation de test non autorisee."
    }
    if (Get-Process -Name visionary -ErrorAction SilentlyContinue) {
        throw 'Ferme Visionary avant de compiler.'
    }
    if ($Action -eq 'All' -and -not (Test-Path -LiteralPath $target -PathType Leaf)) {
        throw "NRO installe introuvable, aucun nouveau chemin cree : $target"
    }

    Write-Host 'Compilation de Visionary...'
    & cargo +stable build --release --locked
    Test-LastExit 'compilation Windows'

    if ($repo -notmatch '^[A-Za-z]:\\') {
        throw "Le dossier source doit etre sur un disque Windows : $repo"
    }
    $linuxRepo = '/mnt/' + $repo.Substring(0, 1).ToLowerInvariant() + '/' + $repo.Substring(3).Replace('\', '/')
    $quotedRepo = "'" + $linuxRepo.Replace("'", "'\''") + "'"
    Write-Host 'Compilation du plugin Skyline...'
    & wsl.exe -- bash -lc "cd $quotedRepo && python3 plugins/slight_replica/tools/build_plugin.py"
    Test-LastExit 'compilation Skyline'

    if (-not (Test-Path -LiteralPath $builtNro -PathType Leaf)) {
        throw "NRO compile introuvable : $builtNro"
    }
    if ($Action -eq 'Build') {
        Write-Host "Compilation terminee. NRO : $builtNro"
        return
    }

    if (Get-Process -Name Ryujinx -ErrorAction SilentlyContinue) {
        [void](Read-Host 'Compilation terminee. Ferme Ryujinx, puis appuie sur Entree pour deployer le NRO')
        if (Get-Process -Name Ryujinx -ErrorAction SilentlyContinue) {
            throw 'Ryujinx est encore ouvert : aucun NRO remplace.'
        }
    }

    $backupDir = Join-Path $sdDir 'backups'
    if (-not (Test-Path -LiteralPath $sdDir -PathType Container)) {
        throw "Dossier Visionary introuvable : $sdDir"
    }
    New-Item -ItemType Directory -Path $backupDir -Force | Out-Null
    $shortCommit = (& git rev-parse --short HEAD).Trim()
    Test-LastExit 'git rev-parse'
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
    $backup = Join-Path $backupDir ("lib_effect_viewer_" + $stamp + "_" + $shortCommit + ".nro")

    if (Test-Path -LiteralPath $backup) {
        throw "Sauvegarde deja presente, aucun ecrasement : $backup"
    }
    Write-Host "Sauvegarde : $backup"
    Copy-Item -LiteralPath $target -Destination $backup -ErrorAction Stop
    $originalHash = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash
    if ((Get-FileHash -LiteralPath $backup -Algorithm SHA256).Hash -ne $originalHash) {
        throw 'La verification de la sauvegarde a echoue. Aucun deploiement.'
    }

    $builtHash = (Get-FileHash -LiteralPath $builtNro -Algorithm SHA256).Hash
    $temporary = "$target.tmp.$([guid]::NewGuid().ToString('N'))"
    try {
        Copy-Item -LiteralPath $builtNro -Destination $temporary -ErrorAction Stop
        if ((Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash -ne $builtHash) {
            throw 'Le NRO temporaire ne correspond pas au NRO compile.'
        }
        Move-Item -LiteralPath $temporary -Destination $target -Force -ErrorAction Stop
        if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $builtHash) {
            throw 'Le NRO installe ne correspond pas au NRO compile.'
        }
    }
    catch {
        Copy-Item -LiteralPath $backup -Destination $target -Force -ErrorAction Stop
        throw
    }
    finally {
        Remove-Item -LiteralPath $temporary -ErrorAction SilentlyContinue
    }
    Write-Host 'Deploiement termine : seul le NRO Visionary a ete remplace.'
    Write-Host "SHA-256 : $builtHash"
}
finally {
    Pop-Location
}

[void](Read-Host 'Demarre Ryujinx et Smash, puis appuie sur Entree pour lancer Visionary')
Start-Visionary

# Installe Microsoft Foundry Local, le moteur NPU par défaut de morph-npu.
#
# Foundry Local utilise le NPU Snapdragon par le fournisseur QNN signé par
# Qualcomm : ni mode test, ni Secure Boot à couper. Le paquet s'installe
# d'abord pour l'utilisateur courant ; si Windows exige l'administrateur, le
# script se relance avec la demande d'autorisation (UAC).

param([switch]$Admin)

$args_winget = @(
    "install", "--id", "Microsoft.FoundryLocal", "--exact",
    "--accept-package-agreements", "--accept-source-agreements"
)

if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    Write-Host "winget est absent. Installez « Programme d'installation d'application » depuis le Microsoft Store, ou Foundry Local depuis https://aka.ms/foundry-local-installer" -ForegroundColor Red
    exit 1
}

if (Get-Command foundry -ErrorAction SilentlyContinue) {
    Write-Host "Foundry Local est déjà installé." -ForegroundColor Green
} else {
    winget @args_winget
    if ($LASTEXITCODE -ne 0) {
        if ($Admin) {
            Write-Host "L'installation a échoué même en administrateur (code $LASTEXITCODE)." -ForegroundColor Red
            exit 1
        }
        Write-Host "Windows demande l'administrateur : acceptez la demande qui s'ouvre." -ForegroundColor Yellow
        Start-Process -FilePath powershell.exe -Verb RunAs -ArgumentList @(
            "-NoProfile", "-ExecutionPolicy", "Bypass", "-NoExit",
            "-File", "`"$PSCommandPath`"", "-Admin"
        )
        exit 0
    }
}

Write-Host ""
Write-Host "Préparation du NPU (téléchargement du fournisseur Qualcomm QNN, une seule fois)…"
$foundry = Get-Command foundry -ErrorAction SilentlyContinue
if (-not $foundry) {
    $foundry = Join-Path $env:LOCALAPPDATA "Microsoft\WindowsApps\foundry.exe"
}
& $foundry model list
Write-Host ""
Write-Host "Terminé. Dans Locaryn, lancez le moteur « NPU Snapdragon »." -ForegroundColor Green

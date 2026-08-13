param(
    [string]$BackupDir = "$env:USERPROFILE\Desktop\bal-server-backups",
    [int]$RetentionDays = 30,
    [switch]$Restore,
    [string]$RestorePath = "",
    [string]$SourcePath = "/home/umbrel/umbrel/app-data/bal-umbrel/data",
    [string]$TempDir = "/tmp/bal-backup"
)

# =============================================================================
# Secrets are NEVER hardcoded here. Provide them via environment variables
# (or you will be prompted interactively):
#
#   $env:UMBREL_HOST      = "umbrel.local"           # optional, defaults below
#   $env:UMBREL_USER      = "umbrel"                 # optional, defaults below
#   $env:UMBREL_SSH_HOSTKEY = "ssh-ed25519 255 SHA256:..."  # plink -hostkey value
#   $env:UMBREL_SUDO_PW   = "<sudo password>"        # prompted if not set
#
# Example (PowerShell, current session only — not persisted):
#   $env:UMBREL_SUDO_PW = Read-Host -AsSecureString | ConvertFrom-SecureString -AsPlainText
#   .\scripts\backup-umbrel.ps1
# =============================================================================

$hostname = if ($env:UMBREL_HOST) { $env:UMBREL_HOST } else { "umbrel.local" }
$user     = if ($env:UMBREL_USER) { $env:UMBREL_USER } else { "umbrel" }
$sshHostKey = $env:UMBREL_SSH_HOSTKEY   # may be empty; plink will then prompt/verify

if ([string]::IsNullOrEmpty($env:UMBREL_SUDO_PW)) {
    $secure = Read-Host -Prompt "Umbrel sudo password" -AsSecureString
    $Password = [System.Runtime.InteropServices.Marshal]::PtrToStringAuto(
        [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure))
} else {
    $Password = $env:UMBREL_SUDO_PW
}

if ([string]::IsNullOrEmpty($Password)) {
    Write-Error "No sudo password provided (set `$env:UMBREL_SUDO_PW or enter it when prompted)."; exit 1
}

# Build plink/pscp host-key args only if a host key was supplied.
$hostKeyArgs = @()
if (-not [string]::IsNullOrEmpty($sshHostKey)) { $hostKeyArgs = @("-hostkey", $sshHostKey) }

if ($Restore) {
    if (-not $RestorePath -or -not (Test-Path $RestorePath)) {
        Write-Error "Specify a valid backup path with -RestorePath"; exit 1
    }
    Write-Host "Restoring from: $RestorePath"
    $restoreFile = Join-Path $RestorePath "bal.db"
    if (Test-Path $restoreFile) {
        & plink -ssh -batch @hostKeyArgs -pw $Password "$user@$hostname" "echo '$Password' | sudo -S mkdir -p $TempDir"
        & pscp -scp @hostKeyArgs -pw $Password $restoreFile "${user}@${hostname}:$TempDir/bal.db"
        & plink -ssh -batch @hostKeyArgs -pw $Password "$user@$hostname" "echo '$Password' | sudo -S docker cp $TempDir/bal.db bal-will-server:/data/bal.db && echo DB restored OK"
    }
    Write-Host "Restore complete"; exit 0
}

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$backupFolder = Join-Path $BackupDir $timestamp
New-Item -ItemType Directory -Path $backupFolder -Force | Out-Null
Write-Host "Backup in progress... ($timestamp)"

# 1. Copy files via sudo to a readable temp dir
& plink -ssh -batch @hostKeyArgs -pw $Password "$user@$hostname" "echo '$Password' | sudo -S rm -rf $TempDir; echo '$Password' | sudo -S mkdir -p $TempDir; echo '$Password' | sudo -S cp -a $SourcePath/. $TempDir/; echo '$Password' | sudo -S chmod -R 777 $TempDir; echo PREP_OK"

if ($LASTEXITCODE -ne 0) { Write-Error "Remote preparation failed"; exit 1 }

# 2. Fetch from temp dir via SCP
& pscp -scp -unsafe @hostKeyArgs -pw $Password -r "${user}@${hostname}:$TempDir/*" "$backupFolder\"

if ($LASTEXITCODE -eq 0) {
    Write-Host "Backup complete: $backupFolder"
    $limit = (Get-Date).AddDays(-$RetentionDays)
    Get-ChildItem $BackupDir -Directory | Where-Object { $_.CreationTime -lt $limit } | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    # Cleanup remote temp
    & plink -ssh -batch @hostKeyArgs -pw $Password "$user@$hostname" "echo '$Password' | sudo -S rm -rf $TempDir"
} else {
    Write-Error "Backup FAILED (code: $LASTEXITCODE)"; exit 1
}

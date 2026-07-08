param(
    [string]$RemoteHost = "rartiushkov@35.226.240.198",
    [string]$RemoteDir = "/opt/hyperion-mesh",
    [string]$StageDir = "~/hyperion-binary-stage",
    [string]$SshKeyPath = "$HOME\.ssh\id_ed25519",
    [string]$ArtifactDir = "C:\Users\r.artyshkov\Downloads\hyperion-backend-linux-x64"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$requiredFiles = @(
    "hyperion_grpc_server",
    "hyperion_card_webhook",
    "deploy_binary.sh",
    "hyperion-grpc.service",
    "hyperion-card-webhook.service"
)

function Write-Log {
    param([string]$Message)
    Write-Host "[hyperion-binary-deploy] $Message"
}

function Invoke-RemoteSsh {
    param([string]$Command)
    & ssh -i $SshKeyPath $RemoteHost $Command
    if ($LASTEXITCODE -ne 0) {
        throw "Remote SSH command failed."
    }
}

if (-not (Test-Path $ArtifactDir)) {
    throw "ArtifactDir not found: $ArtifactDir"
}

foreach ($file in $requiredFiles) {
    $path = Join-Path $ArtifactDir $file
    if (-not (Test-Path $path)) {
        throw "Required artifact file is missing: $path"
    }
}

Write-Log "Preparing remote stage directory $StageDir."
Invoke-RemoteSsh "rm -rf $StageDir && mkdir -p $StageDir"

$sources = $requiredFiles | ForEach-Object { Join-Path $ArtifactDir $_ }
Write-Log "Uploading prebuilt Linux artifact files."
& scp -i $SshKeyPath @sources "${RemoteHost}:${StageDir}/"
if ($LASTEXITCODE -ne 0) {
    throw "Remote copy failed."
}

$remoteDeploy = @'
set -euo pipefail
sudo mkdir -p __REMOTE_DIR__/bin __REMOTE_DIR__/deploy __REMOTE_DIR__/traces
sudo cp __STAGE_DIR__/hyperion_grpc_server __REMOTE_DIR__/bin/
sudo cp __STAGE_DIR__/hyperion_card_webhook __REMOTE_DIR__/bin/
sudo cp __STAGE_DIR__/deploy_binary.sh __REMOTE_DIR__/deploy/
sudo cp __STAGE_DIR__/hyperion-grpc.service __REMOTE_DIR__/deploy/
sudo cp __STAGE_DIR__/hyperion-card-webhook.service __REMOTE_DIR__/deploy/
cd __REMOTE_DIR__
sudo bash deploy/deploy_binary.sh
'@

$remoteDeploy = $remoteDeploy.Replace("__STAGE_DIR__", $StageDir).Replace("__REMOTE_DIR__", $RemoteDir)

Write-Log "Installing binaries and restarting services."
Invoke-RemoteSsh $remoteDeploy

Write-Log "Binary deployment finished."

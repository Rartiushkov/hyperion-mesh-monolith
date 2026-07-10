param(
    [string]$RemoteHost = "rartiushkov@35.226.240.198",
    [string]$RemoteDir = "/opt/hyperion-mesh",
    [string]$StageDir = "~/hyperion-mesh-stage",
    [string]$SshKeyPath = "$HOME\.ssh\id_ed25519"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$files = @(
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    ".cargo",
    "benches",
    "raman-core",
    "proto",
    "src",
    "deploy"
)

function Write-Log {
    param([string]$Message)
    Write-Host "[hyperion-remote-deploy] $Message"
}

function Invoke-RemoteSsh {
    param([string]$Command)
    & ssh -i $SshKeyPath $RemoteHost $Command
    if ($LASTEXITCODE -ne 0) {
        throw "Remote SSH command failed."
    }
}

function Invoke-RemoteCopy {
    $sources = $files | ForEach-Object { Join-Path $repoRoot $_ }
    & scp -i $SshKeyPath -r @sources "${RemoteHost}:${StageDir}/"
    if ($LASTEXITCODE -ne 0) {
        throw "Remote copy failed."
    }
}

Write-Log "Creating remote staging directory $StageDir."
Invoke-RemoteSsh "rm -rf $StageDir && mkdir -p $StageDir"

Write-Log "Uploading Hyperion build payload."
Push-Location $repoRoot
try {
    Invoke-RemoteCopy
} finally {
    Pop-Location
}

$remoteBuild = @'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev protobuf-compiler cron
if ! command -v cargo >/dev/null 2>&1; then
  curl https://sh.rustup.rs -sSf | sh -s -- -y
fi
source "$HOME/.cargo/env"
cd __STAGE_DIR__
cargo build --release --bin hyperion_grpc_server --bin hyperion_card_webhook
sudo mkdir -p __REMOTE_DIR__/bin __REMOTE_DIR__/deploy __REMOTE_DIR__/traces
sudo cp target/release/hyperion_grpc_server __REMOTE_DIR__/bin/
sudo cp target/release/hyperion_card_webhook __REMOTE_DIR__/bin/
sudo cp deploy/deploy_binary.sh __REMOTE_DIR__/deploy/
sudo cp deploy/hyperion-grpc.service __REMOTE_DIR__/deploy/
sudo cp deploy/hyperion-card-webhook.service __REMOTE_DIR__/deploy/
cd __REMOTE_DIR__
sudo bash deploy/deploy_binary.sh
'@

$remoteBuild = $remoteBuild.Replace("__STAGE_DIR__", $StageDir).Replace("__REMOTE_DIR__", $RemoteDir)

Write-Log "Building on remote host and deploying services."
Invoke-RemoteSsh $remoteBuild

Write-Log "Hyperion remote deployment finished."

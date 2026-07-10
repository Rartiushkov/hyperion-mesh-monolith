`remote_deploy_source.ps1` mirrors the `hft_arbitrage_core` deploy pattern:

- run from the Windows machine
- upload the minimal Hyperion source payload over `ssh/scp`
- build Linux binaries on the remote host
- promote the binaries + systemd units into `/opt/hyperion-mesh`
- run `deploy/deploy_binary.sh`

Example:

```powershell
powershell -ExecutionPolicy Bypass -File .\tools\remote_deploy_source.ps1 `
  -RemoteHost "rartiushkov@35.226.240.198" `
  -SshKeyPath "$HOME\.ssh\id_ed25519"
```

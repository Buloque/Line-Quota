# Gera dist\LineQuota-Setup-<versao>.exe: o instalador com janela propria (instala e atualiza), em Rust.
# Ordem: 1) compila o app; 2) compila o instalador embutindo o app; 3) copia para dist\.
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)
$env:Path += ";$env:USERPROFILE\.cargo\bin"
cargo build --release -p line-quota
if ($LASTEXITCODE -ne 0) { throw 'falha no build do app' }
$env:LQ_PAYLOAD = (Resolve-Path target\release\line-quota.exe).Path
cargo build --release -p line-quota-setup
if ($LASTEXITCODE -ne 0) { throw 'falha no build do instalador' }
$ver = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
New-Item -ItemType Directory -Force dist | Out-Null
$setup = "dist\LineQuota-Setup-$ver.exe"
Copy-Item -Force target\release\LineQuota-Setup.exe $setup
Get-Item $setup | Select-Object Name, @{n='MB';e={[math]::Round($_.Length/1MB,2)}}, @{n='SHA256';e={(Get-FileHash $_.FullName).Hash}}

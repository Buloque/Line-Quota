# Gera dist\LineQuota-Setup-<versao>.exe: o instalador (duplo clique, sem PowerShell/CMD para o usuario).
# O mesmo .exe vira o assistente de instalacao quando o nome do arquivo contem "Setup".
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)
$env:Path += ";$env:USERPROFILE\.cargo\bin"
cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'falha no build' }
$ver = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
New-Item -ItemType Directory -Force dist | Out-Null
$setup = "dist\LineQuota-Setup-$ver.exe"
Copy-Item -Force target\release\line-quota.exe $setup
Get-Item $setup | Select-Object Name, Length

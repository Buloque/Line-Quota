# Gera dist\LineQuota-<versao>.zip com o programa e o atalho de instalação.
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)
$env:Path += ";$env:USERPROFILE\.cargo\bin"
cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'falha no build' }
$ver = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
$out = "dist\LineQuota-$ver"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item target\release\line-quota.exe $out
@'
@echo off
cd /d "%~dp0"
line-quota.exe --install
'@ | Set-Content -Encoding ascii "$out\Instalar Line Quota.cmd"
@'
Line Quota - instalacao
1. De dois cliques em "Instalar Line Quota.cmd".
2. O widget aparece na barra de tarefas; clique com o botao direito para configurar.
Para remover: Configuracoes do Windows > Aplicativos instalados > Line Quota.
Instalacao silenciosa: line-quota.exe --install --quiet [--no-autostart]
Remocao completa (apaga tambem a configuracao): line-quota.exe --uninstall --purge
'@ | Set-Content -Encoding ascii "$out\LEIAME.txt"
Compress-Archive -Force -Path "$out\*" -DestinationPath "dist\LineQuota-$ver.zip"
Get-Item "dist\LineQuota-$ver.zip" | Select-Object Name, Length

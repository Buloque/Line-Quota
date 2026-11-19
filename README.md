# Line Quota

Widget nativo para Windows (Rust) que mostra, na barra de tarefas, os limites de 5 horas e semanal do
**Claude Code**, **Codex** e **OpenCode**, o que cada agente está fazendo e avisos de limite/tarefa concluída.

## Instalar
1. Gere o pacote: `powershell -File scripts\package.ps1` (cria `dist\LineQuota-<versão>.zip`).
2. Extraia e dê dois cliques em **Instalar Line Quota.cmd**.

A instalação é por usuário (sem administrador): copia o programa para `%LOCALAPPDATA%\Programs\LineQuota`,
configura os hooks do Claude Code (`~/.claude/settings.json`, com backup), liga o início com o Windows e cria
a entrada em *Aplicativos instalados*. Opções: `--install --quiet --no-autostart`.

## Remover
*Configurações do Windows → Aplicativos instalados → Line Quota*, ou `line-quota.exe --uninstall [--purge]`
(`--purge` apaga também a configuração em `%APPDATA%\LineQuota`).

## Uso
Botão direito no widget → **Configurações…**. Clique esquerdo num provedor abre a árvore Projeto → Chats →
o que está fazendo. Arraste para posicionar ao longo da barra.

# Line Quota

Widget nativo para Windows (Rust) que mostra, na barra de tarefas, os limites de 5 horas, semanal (e mensal, no
Codex gratuito) do **Claude Code**, **Codex** e **OpenCode**, o que cada agente está fazendo e avisos de
limite/tarefa concluída. Nenhum dos três programas é obrigatório.

## Instalar
1. Gere o instalador: `powershell -File scripts\package.ps1` (cria `dist\LineQuota-Setup-<versão>.exe`).
2. Dê dois cliques no **LineQuota-Setup-\<versão\>.exe**, escolha se inicia com o Windows e clique em *Instalar*.

A instalação é por usuário (sem administrador): copia o programa para `%LOCALAPPDATA%\Programs\LineQuota`,
configura os hooks do Claude Code (`~/.claude/settings.json`, com backup — só se o Claude Code existir),
liga o início com o Windows e cria a entrada em *Aplicativos instalados*. Na primeira execução, só os agentes
encontrados no computador ficam ligados. Para scripts: `line-quota.exe --install --quiet [--no-autostart]`.

## Remover
*Configurações do Windows → Aplicativos instalados → Line Quota → Desinstalar*. O desinstalador pergunta se
deve apagar também as configurações (**purge**). Pela linha de comando: `line-quota.exe --uninstall [--quiet] [--purge]`
(`--purge` apaga também `%APPDATA%\LineQuota` e `%LOCALAPPDATA%\LineQuota`).

## Uso
- Botão direito no widget: **Destravar/Travar barra**, **Atualizar**, **Abrir configurações** e **Sair**.
  Todo o resto fica na janela de configurações.
- Clique esquerdo num provedor abre a árvore Projeto → Chat (nome do chat) → pedido / o que está fazendo.
- A posição vem **travada** (um clique sem querer não move o widget). Para mudar de lugar: *Destravar barra* e
  arraste ao longo da barra; depois *Travar barra* de novo.
- Fonte padrão: Calibri (mude em *Configurações → Fonte*).
- Gráficos: barra horizontal, vertical, círculo ou blocos, com ou sem número e com ou sem os rótulos (5h / 7d / 30d). Codex no plano gratuito mostra o limite mensal (`30d`) automaticamente.
- *Configurações → Posição*: onde ancorar, monitor e **Travar a posição na barra**. Modo automático (só o que está aberto) e ocultar o widget vêm ligados em instalações novas.

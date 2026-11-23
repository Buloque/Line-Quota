use crate::model::ProviderId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProviderConfig {
    pub enabled: bool,
    pub show_five_hour: bool,
    pub show_weekly: bool,
    pub show_activity: bool,
    pub refresh_idle_secs: u64,
    pub refresh_active_secs: u64,
    /// Nomes dos executáveis que indicam que o programa está aberto (modo automático).
    pub processes: Vec<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self { enabled: true, show_five_hour: true, show_weekly: true, show_activity: true,
               refresh_idle_secs: 300, refresh_active_secs: 60, processes: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AlertConfig {
    pub enabled: bool,
    /// Limiares de uso (%) que disparam aviso, em ordem crescente.
    pub thresholds: Vec<f32>,
    pub toast: bool,
    pub sound: bool,
    pub notify_reset: bool,
    pub notify_task_done: bool,
    pub notify_needs_input: bool,
    /// Tarefas mais curtas que isto (segundos) não geram aviso de "concluída".
    pub task_done_min_secs: i64,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self { enabled: true, thresholds: vec![80.0, 90.0, 95.0], toast: true, sound: false,
               notify_reset: true, notify_task_done: true, notify_needs_input: true, task_done_min_secs: 30 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    /// Ao lado da área de notificação (padrão).
    #[default] BesideTray,
    /// Início da barra de tarefas.
    TaskbarStart,
    /// Centro da barra de tarefas.
    TaskbarCenter,
    /// Posição livre ao longo da barra (arrastada pelo usuário, só no eixo da barra).
    Custom,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode { Number, Bar, #[default] Both }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LabelStyle { #[default] Icon, Text, /// Sem identificação do provedor (só os limites).
    None }

/// Formato do gráfico de cada limite.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum BarStyle {
    /// Barra horizontal.
    #[default] Horizontal,
    /// Barra vertical (em pé).
    Vertical,
    /// Anel (círculo que se preenche).
    Ring,
    /// Cinco blocos (cada um vale 20%).
    Blocks,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiConfig {
    pub compact: bool,
    pub show_remaining: bool,
    pub exact_reset_time: bool,
    pub autostart: bool,
    /// Número, barra de progresso ou ambos.
    pub display: DisplayMode,
    /// Formato do gráfico (barra horizontal, vertical, círculo ou blocos).
    pub bar_style: BarStyle,
    /// Mostra os rótulos de texto dos limites (5h / 7d / 30d). Desligado: só os gráficos.
    pub show_tags: bool,
    /// Ícone do provedor ou texto (Cl/Cx).
    pub labels: LabelStyle,
    pub anchor: Anchor,
    /// 0 = monitor da barra principal; N >= 1 = N-ésimo monitor (ordenado da esquerda p/ direita).
    pub monitor: u32,
    /// Distância (px) desde o início da barra, usada com `anchor = "custom"`.
    pub bar_offset: i32,
    /// Fundo transparente (só as informações). Desligado = fundo sólido.
    pub transparent: bool,
    /// Modo automático: mostra só os programas que estão abertos.
    pub auto_mode: bool,
    /// No modo automático, oculta o widget quando nenhum programa está aberto
    /// (desligado = mostra todos).
    pub auto_hide_when_none: bool,
    /// Posição travada: o widget não é arrastado por clique acidental (destrave pelo menu).
    pub locked: bool,
    /// Fonte do widget (se não estiver instalada, usa Segoe UI).
    pub font: String,
    /// Painel em árvore: só chats em execução ou ativos nesta janela (minutos).
    pub chat_window_mins: u32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { compact: true, show_remaining: false, exact_reset_time: false, autostart: true,
               display: DisplayMode::Both, bar_style: BarStyle::Horizontal, show_tags: true, labels: LabelStyle::Icon, anchor: Anchor::BesideTray,
               monitor: 0, bar_offset: 0, transparent: true, auto_mode: true, auto_hide_when_none: true,
               locked: true, font: "Calibri".into(), chat_window_mins: 30 }
    }
}

/// Opções de contas/login.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct AccountConfig {
    /// Pede o escopo completo no login do Claude (só se o mínimo for recusado).
    pub claude_full_scope: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub account: AccountConfig,
    pub claude: ProviderConfig,
    pub codex: ProviderConfig,
    pub opencode: ProviderConfig,
    pub alerts: AlertConfig,
    pub ui: UiConfig,
}

const PROCESS_DEFAULTS: [(ProviderId, &[&str]); 3] = [
    (ProviderId::Claude, &["claude.exe"]),
    (ProviderId::Codex, &["codex.exe"]),
    (ProviderId::OpenCode, &["opencode.exe", "opencode-cli.exe"]),
];

impl Default for Config {
    fn default() -> Self {
        let mut c = Self {
            account: AccountConfig::default(), claude: ProviderConfig::default(), codex: ProviderConfig::default(),
            opencode: ProviderConfig::default(), alerts: AlertConfig::default(), ui: UiConfig::default(),
        };
        c.fill_defaults();
        c
    }
}

impl Config {
    pub fn provider(&self, id: ProviderId) -> &ProviderConfig {
        match id {
            ProviderId::Claude => &self.claude,
            ProviderId::Codex => &self.codex,
            ProviderId::OpenCode => &self.opencode,
        }
    }

    pub fn providers_mut(&mut self) -> [&mut ProviderConfig; 3] { [&mut self.claude, &mut self.codex, &mut self.opencode] }

    /// Preenche os nomes de processo padrão quando o arquivo de config antigo não os tem.
    pub fn fill_defaults(&mut self) {
        for (id, names) in PROCESS_DEFAULTS {
            let p = match id {
                ProviderId::Claude => &mut self.claude,
                ProviderId::Codex => &mut self.codex,
                ProviderId::OpenCode => &mut self.opencode,
            };
            if p.processes.is_empty() { p.processes = names.iter().map(|s| s.to_string()).collect(); }
        }
    }

    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        let mut c: Self = toml::from_str(s.trim_start_matches('\u{feff}'))?;
        c.fill_defaults();
        Ok(c)
    }
    pub fn to_toml(&self) -> String { toml::to_string_pretty(self).unwrap_or_default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_toml_uses_defaults() {
        let c = Config::from_toml("[alerts]\nthresholds=[70.0]").unwrap();
        assert_eq!(c.alerts.thresholds, vec![70.0]);
        assert!(c.claude.enabled);
    }

    #[test]
    fn old_config_without_new_fields_gets_process_defaults() {
        let c = Config::from_toml("[claude]\nenabled = true\n[ui]\ndisplay = \"bar\"").unwrap();
        assert_eq!(c.claude.processes, vec!["claude.exe"]);
        assert!(c.opencode.processes.contains(&"opencode-cli.exe".to_string()));
        assert!(c.ui.transparent && c.ui.auto_mode);
    }

    #[test]
    fn custom_processes_are_kept() {
        let c = Config::from_toml("[codex]\nprocesses = [\"meucodex.exe\"]").unwrap();
        assert_eq!(c.codex.processes, vec!["meucodex.exe"]);
    }

    #[test]
    fn new_defaults() {
        let c = Config::default();
        assert!(c.ui.locked && c.ui.auto_mode && c.ui.auto_hide_when_none && c.ui.show_tags);
        assert_eq!((c.ui.font.as_str(), c.ui.bar_style), ("Calibri", BarStyle::Horizontal));
        // configs antigas (com campos removidos e sem os novos) continuam carregando
        let old = Config::from_toml("[ui]\nanchor = \"custom\"\nopencode_free_icon_only = false\nbar_style = \"ring\"\n[claude]\nshow_monthly = true").unwrap();
        assert!(old.ui.locked && old.ui.show_tags);
        assert_eq!(old.ui.bar_style, BarStyle::Ring);
    }

    #[test]
    fn roundtrip() {
        let c = Config::default();
        assert_eq!(Config::from_toml(&c.to_toml()).unwrap(), c);
    }
}

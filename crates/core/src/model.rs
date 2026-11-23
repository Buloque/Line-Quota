use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Claude,
    Codex,
    OpenCode,
    Cursor,
    Antigravity,
}

impl ProviderId {
    /// Todos os provedores, na ordem padrão da barra.
    pub const ALL: [ProviderId; 5] = [ProviderId::Claude, ProviderId::Codex, ProviderId::OpenCode, ProviderId::Cursor, ProviderId::Antigravity];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MeterKind {
    FiveHour,
    Weekly,
    /// Janela de ~30 dias (ex.: Codex no plano gratuito).
    Monthly,
    /// Antigravity: cota semanal dos modelos Claude (e abertos, como GPT-OSS).
    Claude,
    /// Antigravity: cota de 5 horas dos modelos Claude (e abertos).
    Claude5h,
    /// Antigravity: cota semanal dos modelos Gemini.
    Gemini,
}

/// Uma janela de limite. `used_pct` vai de 0 a 100.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Meter {
    pub kind: MeterKind,
    pub used_pct: f32,
    /// Instante do reset (segundos Unix), se conhecido.
    pub resets_at: Option<i64>,
    /// Valor estimado (ex.: OpenCode sem API oficial).
    pub estimated: bool,
}

impl Meter {
    pub fn remaining_pct(&self) -> f32 {
        (100.0 - self.used_pct).clamp(0.0, 100.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AgentState {
    #[default]
    Idle,
    /// Tarefa em andamento.
    Working,
    /// Agente aguardando resposta/permissão do usuário.
    NeedsInput,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub provider: ProviderId,
    pub meters: Vec<Meter>,
}

impl Usage {
    pub fn meter(&self, kind: MeterKind) -> Option<&Meter> {
        self.meters.iter().find(|m| m.kind == kind)
    }
}

/// O que um agente está fazendo (para o painel que abre ao clicar no provedor).
/// Tempos em segundos Unix; o texto é curto e vem dos hooks/logs locais.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDetail {
    /// Nome da pasta do projeto.
    pub project: Option<String>,
    /// Último pedido do usuário (resumido).
    pub prompt: Option<String>,
    /// Última ferramenta usada (ex.: "Edit") e o alvo (ex.: "win.rs").
    pub tool: Option<String>,
    pub target: Option<String>,
    pub tool_ts: Option<i64>,
    /// Quando a tarefa atual começou.
    pub since: Option<i64>,
    /// Mensagem de aviso quando está aguardando resposta/permissão.
    pub notice: Option<String>,
    /// Último sinal de vida.
    pub updated: Option<i64>,
}

/// Um chat/sessão de um provedor (nó da árvore do painel).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatInfo {
    pub id: String,
    /// Pasta do projeto (nome curto); `None` = chat avulso, sem projeto.
    pub project: Option<String>,
    pub title: String,
    pub state: AgentState,
    pub detail: AgentDetail,
}

/// Cache simples com validade, para leituras locais um pouco mais caras (a cada tick de 2 s).
pub struct TtlCache<T> {
    ttl_secs: u64,
    slot: std::sync::Mutex<Option<(std::time::Instant, T)>>,
}

impl<T: Clone> TtlCache<T> {
    pub fn new(ttl_secs: u64) -> Self { Self { ttl_secs, slot: std::sync::Mutex::new(None) } }
    pub fn get_or(&self, f: impl FnOnce() -> T) -> T {
        let mut s = self.slot.lock().unwrap();
        if let Some((t, v)) = s.as_ref() {
            if t.elapsed().as_secs() < self.ttl_secs { return v.clone(); }
        }
        let v = f();
        *s = Some((std::time::Instant::now(), v.clone()));
        v
    }
}

/// Nome curto do projeto a partir da pasta de trabalho; `None` se for a pasta do usuário (chat avulso).
pub fn project_from_dir(dir: &str) -> Option<String> {
    let norm = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_lowercase();
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let d = norm(dir);
    if d.is_empty() || (!home.is_empty() && d == norm(&home)) { return None; }
    dir.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().filter(|n| !n.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod project_tests {
    use super::*;

    #[test]
    fn project_name_from_dir() {
        assert_eq!(project_from_dir("C:\\a\\Meu App\\").as_deref(), Some("Meu App"));
        assert_eq!(project_from_dir("").as_deref(), None);
        if let Ok(h) = std::env::var("USERPROFILE") { assert_eq!(project_from_dir(&h), None); }
    }
}

use crate::model::{AgentDetail, AgentState, ChatInfo, ProviderId, Usage};

/// Contrato de cada provedor. Adicionar um provedor = implementar esta trait.
pub trait Provider: Send {
    fn id(&self) -> ProviderId;
    /// Busca o uso atual (bloqueante; pode ser lento, chamado em thread de polling).
    fn fetch(&self) -> Result<Usage, String>;
    /// Estado de atividade (barato: só leitura local; chamado com frequência).
    fn activity(&self) -> AgentState { AgentState::Idle }
    /// true se `fetch` só lê arquivos locais (barato): é chamado a cada tick, sem esperar o intervalo de rede.
    fn local_only(&self) -> bool { false }
    /// O que o agente está fazendo agora (best-effort; leitura local barata).
    fn detail(&self) -> Option<AgentDetail> { None }
    /// Chats/sessões recentes (para a árvore do painel). Leitura local, barata (use cache se não for).
    fn chats(&self) -> Vec<ChatInfo> { Vec::new() }
}

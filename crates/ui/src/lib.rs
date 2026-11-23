//! UI: partes puras (layout, view) testáveis em qualquer SO; `win` é só Windows.
pub mod layout;
pub mod toast;
pub mod view;
#[cfg(windows)]
pub mod win;

/// Ações de conta que a UI dispara sem conhecer os provedores (implementado pelo app).
pub trait Account: Send + Sync {
    fn logged_in(&self) -> bool;
    /// Inicia o login e devolve a URL a abrir no navegador.
    fn login_url(&self, full_scope: bool) -> Result<String, String>;
    /// Conclui o login com o texto copiado da página de autorização.
    fn complete(&self, pasted: &str) -> Result<(), String>;
    fn logout(&self);
    /// Cursor: há um cookie de sessão salvo (para mostrar o limite do plano)?
    fn cursor_connected(&self) -> bool { false }
    /// Cursor: guarda o cookie `WorkosCursorSessionToken` copiado do navegador.
    fn cursor_set_cookie(&self, _pasted: &str) -> Result<(), String> { Err("Sem suporte".into()) }
    fn cursor_clear(&self) {}
    /// O texto parece o cookie de sessão do Cursor?
    fn cursor_looks_like_cookie(&self, _text: &str) -> bool { false }
}

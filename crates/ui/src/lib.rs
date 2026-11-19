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
}

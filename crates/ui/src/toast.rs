//! XML do toast do Windows (puro, testável): título, texto, logo e barra de progresso.
use lq_core::notify::Notification;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// `logo_url` é uma URL `file:///...` para um PNG. `sound` liga o som padrão de notificação.
pub fn toast_xml(n: &Notification, logo_url: &str, sound: bool) -> String {
    let progress = n.progress.as_ref().map(|p| {
        format!(r#"<progress value="{:.3}" title="{}" status="{}" valueStringOverride="{}"/>"#,
            p.fraction.clamp(0.0, 1.0), esc(&p.title), esc(&p.status), esc(&p.status))
    }).unwrap_or_default();
    let audio = if sound { r#"<audio src="ms-winsoundevent:Notification.Default"/>"# } else { r#"<audio silent="true"/>"# };
    format!(
        r#"<toast><visual><binding template="ToastGeneric"><image placement="appLogoOverride" src="{}"/><text>{}</text><text>{}</text>{progress}</binding></visual>{audio}</toast>"#,
        esc(logo_url), esc(&n.title), esc(&n.body)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lq_core::notify::{Progress, Severity};

    #[test]
    fn escapes_and_includes_progress() {
        let mut n = Notification::new("A & B <c>", "x \"y\"", Severity::Warn);
        n.progress = Some(Progress { fraction: 1.7, title: "Limite".into(), status: "90% usado".into() });
        let x = toast_xml(&n, "file:///C:/a b/icon.png", false);
        assert!(x.contains("<text>A &amp; B &lt;c&gt;</text>"));
        assert!(x.contains("x &quot;y&quot;"));
        assert!(x.contains(r#"<progress value="1.000" title="Limite" status="90% usado""#));
        assert!(x.contains(r#"<audio silent="true"/>"#));
        assert!(x.starts_with("<toast>") && x.ends_with("</toast>"));
    }

    #[test]
    fn no_progress_and_sound() {
        let n = Notification::new("t", "b", Severity::Info);
        let x = toast_xml(&n, "file:///i.png", true);
        assert!(!x.contains("<progress"));
        assert!(x.contains("Notification.Default"));
    }
}

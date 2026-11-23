//! Embute o `line-quota.exe` dentro do instalador e grava ícone/versão/manifesto no .exe.
//! Gere o app primeiro (`cargo build --release -p line-quota`); `scripts/package.ps1` faz tudo na ordem certa.
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let payload = std::env::var("LQ_PAYLOAD").map(PathBuf::from)
        .unwrap_or_else(|_| manifest.join("../../target/release/line-quota.exe"));
    let dest = out.join("payload.bin");
    match std::fs::copy(&payload, &dest) {
        Ok(_) => {}
        Err(_) => {
            // Sem o app compilado (ex.: `cargo check`/`cargo test`): instalador vazio, que avisa ao abrir.
            println!("cargo:warning=line-quota.exe não encontrado em {}; instalador sem pacote", payload.display());
            std::fs::write(&dest, []).unwrap();
        }
    }
    println!("cargo:rerun-if-env-changed=LQ_PAYLOAD");
    println!("cargo:rerun-if-changed={}", payload.display());
    println!("cargo:rerun-if-changed=installer.manifest");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set_manifest_file("installer.manifest");
        res.set("FileDescription", "Instalador do Line Quota");
        res.set("ProductName", "Line Quota");
        res.set("CompanyName", "Line Quota");
        res.set("LegalCopyright", "MIT License");
        res.set("InternalName", "LineQuota-Setup");
        res.set("OriginalFilename", "LineQuota-Setup.exe");
        res.compile().expect("falha ao compilar os recursos do Windows (rc.exe)");
    }
}

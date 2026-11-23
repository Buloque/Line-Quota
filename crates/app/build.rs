//! Embute ícone e informações de versão no .exe (nome "Line Quota" no Windows e nas notificações).
fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set_manifest_file("app.manifest");
        res.set("FileDescription", "Line Quota");
        res.set("ProductName", "Line Quota");
        res.set("CompanyName", "Line Quota");
        res.set("LegalCopyright", "MIT License");
        res.set("InternalName", "line-quota");
        res.set("OriginalFilename", "line-quota.exe");
        res.compile().expect("falha ao compilar os recursos do Windows (rc.exe)");
    }
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    println!("cargo:rerun-if-changed=app.manifest");
}

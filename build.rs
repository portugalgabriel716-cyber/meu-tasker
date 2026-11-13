//! Compilando para Windows (lá ou daqui, com MinGW), embute o ícone no .exe. Sem
//! ferramenta de recursos, segue sem ícone.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    // O build.rs roda em quem compila: quem diz o destino é o CARGO_CFG_*.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=sem ícone no .exe: {e}");
        }
    }
}

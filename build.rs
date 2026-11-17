fn main() {
    println!("cargo:rerun-if-changed=assets/quarz.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/quarz.ico");
        resource.set("ProductName", "Quarz");
        resource.set("FileDescription", "Quarz");
        if let Err(e) = resource.compile() {
            println!("cargo:warning=Programmsymbol nicht eingebettet: {e}");
        }
    }
}

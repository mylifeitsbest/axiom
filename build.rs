//! Встраивает иконку и описание в axiom.exe (Windows).

fn main() {
    println!("cargo:rerun-if-changed=assets/axiom.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/axiom.ico")
            .set("FileDescription", "Axiom")
            .set("ProductName", "Axiom")
            .set("CompanyName", "Axiom")
            .set("LegalCopyright", "Axiom contributors");
        if let Err(err) = res.compile() {
            println!("cargo:warning=иконка не встроена: {err}");
        }
    }
}

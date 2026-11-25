//! Встраивает иконку и описание в axiom.exe (Windows).
//!
//! Бета-версия (`AXIOM_BETA_BUILD=N`, её задаёт .github/workflows/beta.yml)
//! получает версию файла `X.Y.Z.N`: по ней более новая копия узнаёт, что
//! установленная старее (src/system/refresh.rs). У выпуска четвёртое число — 0.

fn main() {
    println!("cargo:rerun-if-changed=assets/axiom.ico");
    println!("cargo:rerun-if-env-changed=AXIOM_BETA_BUILD");
    let beta = beta_build();
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/axiom.ico")
            .set("FileDescription", "Axiom")
            .set("ProductName", "Axiom")
            .set("CompanyName", "Axiom")
            .set("LegalCopyright", "Axiom contributors");
        if let Some(n) = beta {
            let part = |name: &str| -> u64 {
                std::env::var(name)
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0)
            };
            let version = (part("CARGO_PKG_VERSION_MAJOR") << 48)
                | (part("CARGO_PKG_VERSION_MINOR") << 32)
                | (part("CARGO_PKG_VERSION_PATCH") << 16)
                | u64::from(n);
            let text = format!("{}-beta.{n}", env!("CARGO_PKG_VERSION"));
            res.set_version_info(winresource::VersionInfo::FILEVERSION, version)
                .set_version_info(winresource::VersionInfo::PRODUCTVERSION, version)
                .set("FileVersion", &text)
                .set("ProductVersion", &text);
        }
        if let Err(err) = res.compile() {
            println!("cargo:warning=иконка не встроена: {err}");
        }
    }
    #[cfg(not(windows))]
    let _ = beta;
}

/// Номер бета-версии: целое от 1 до 65535 (столько вмещает версия файла Windows).
/// Неверное значение — ошибка сборки, а не тихий выпуск под видом теста.
fn beta_build() -> Option<u16> {
    let raw = std::env::var("AXIOM_BETA_BUILD").ok()?;
    match raw.parse::<u16>() {
        Ok(n) if n > 0 && raw.bytes().all(|b| b.is_ascii_digit()) => Some(n),
        _ => panic!("AXIOM_BETA_BUILD должен быть числом от 1 до 65535, а не {raw:?}"),
    }
}

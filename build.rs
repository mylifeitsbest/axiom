//! Встраивает иконку и описание в axiom.exe (Windows). На Linux собирает
//! расширение веб-процесса WebKitGTK для подмены Referer (src/tabs/webext.c).
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
    #[cfg(target_os = "linux")]
    web_extension();
}

/// `src/tabs/webext.c` → `OUT_DIR/axiom-webext.so`; его встраивает src/tabs/identity.rs.
/// Заголовки — из того же пакета разработки WebKitGTK, что нужен самому wry.
#[cfg(target_os = "linux")]
fn web_extension() {
    use std::{env, path::PathBuf, process::Command};

    const SRC: &str = "src/tabs/webext.c";
    const LIB: &str = "webkit2gtk-web-extension-4.1";
    println!("cargo:rerun-if-changed={SRC}");
    for var in ["CC", "CFLAGS", "LDFLAGS", "PKG_CONFIG", "PKG_CONFIG_PATH"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let words = |var: &str| -> Vec<String> {
        env::var(var)
            .unwrap_or_default()
            .split_whitespace()
            .map(String::from)
            .collect()
    };
    let pkg_config = env::var("PKG_CONFIG").unwrap_or_else(|_| "pkg-config".into());
    let flags = match Command::new(&pkg_config)
        .args(["--cflags", "--libs", LIB])
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        Ok(out) => panic!(
            "{pkg_config} не нашёл {LIB} (пакет libwebkit2gtk-4.1-dev или webkit2gtk-4.1): {}",
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(err) => panic!("не запустить {pkg_config}: {err}"),
    };
    // `CC` может быть и с аргументами («ccache gcc»).
    let mut cc = words("CC");
    if cc.is_empty() {
        cc.push("cc".into());
    }
    let Some(out_dir) = env::var_os("OUT_DIR") else {
        panic!("cargo не задал OUT_DIR");
    };
    let out = PathBuf::from(out_dir).join("axiom-webext.so");
    let status = Command::new(&cc[0])
        .args(&cc[1..])
        .args(["-shared", "-fPIC", "-O2", "-Wall"])
        .args(words("CFLAGS"))
        .args(words("LDFLAGS"))
        .arg(SRC)
        .arg("-o")
        .arg(&out)
        .args(flags.split_whitespace())
        .status();
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => panic!("{} не собрал {SRC}: {status}", cc[0]),
        Err(err) => panic!("не запустить {}: {err}", cc[0]),
    }
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

//! Служебный webview интерфейса: раздача файлов `ui/` по протоколу `axiom://`
//! и проверка, что IPC пришёл именно отсюда.

pub mod hotkeys;

use std::borrow::Cow;

use wry::http::{Request, Response, StatusCode, header::CONTENT_TYPE};

/// На Windows wry отдаёт свой протокол как `https://axiom.localhost/`.
#[cfg(windows)]
pub const ORIGIN: &str = "https://axiom.localhost/";
#[cfg(not(windows))]
pub const ORIGIN: &str = "axiom://localhost/";

pub fn url() -> String {
    // Только для разработки: открыть нужный экран сразу (`AXIOM_UI_HASH=settings:look`).
    #[cfg(debug_assertions)]
    if let Ok(hash) = std::env::var("AXIOM_UI_HASH")
        && hash.bytes().all(|b| b.is_ascii_alphanumeric() || b == b':')
    {
        return format!("{ORIGIN}index.html#{hash}");
    }
    format!("{ORIGIN}index.html")
}

/// Источник сообщения — страница интерфейса, а не сайт.
pub fn is_ui_origin(url: &str) -> bool {
    url.starts_with(ORIGIN)
}

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
                   img-src 'self' https: http: data:; font-src 'self'; connect-src 'none'; \
                   base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

struct Asset {
    path: &'static str,
    mime: &'static str,
    #[cfg_attr(debug_assertions, allow(dead_code))]
    bytes: &'static [u8],
}

const ASSETS: &[Asset] = &[
    Asset {
        path: "/index.html",
        mime: "text/html; charset=utf-8",
        bytes: include_bytes!("../../ui/index.html"),
    },
    Asset {
        path: "/styles.css",
        mime: "text/css; charset=utf-8",
        bytes: include_bytes!("../../ui/styles.css"),
    },
    Asset {
        path: "/app.js",
        mime: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../ui/app.js"),
    },
    Asset {
        path: "/icons.js",
        mime: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../ui/icons.js"),
    },
    Asset {
        path: "/settings.js",
        mime: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../ui/settings.js"),
    },
    Asset {
        path: "/downloads.js",
        mime: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../ui/downloads.js"),
    },
    Asset {
        path: "/update.js",
        mime: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../ui/update.js"),
    },
    Asset {
        path: "/translate.js",
        mime: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../ui/translate.js"),
    },
    Asset {
        path: "/snapshot.js",
        mime: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../ui/snapshot.js"),
    },
    Asset {
        path: "/newtab.js",
        mime: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../ui/newtab.js"),
    },
    Asset {
        path: "/tabmenu.js",
        mime: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../ui/tabmenu.js"),
    },
    Asset {
        path: "/applink.js",
        mime: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../ui/applink.js"),
    },
];

pub fn serve(request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = match request.uri().path() {
        "/" | "" => "/index.html",
        p => p,
    };
    // Снимок страницы под окном. Этот протокол есть только у webview интерфейса,
    // поэтому сайты во вкладках снимок не запросят.
    if let Some((mime, bytes)) = crate::tabs::snapshot::serve(path) {
        return respond(StatusCode::OK, mime, Cow::Owned(bytes));
    }
    let Some(asset) = ASSETS.iter().find(|a| a.path == path) else {
        return respond(
            StatusCode::NOT_FOUND,
            "text/plain",
            Cow::Borrowed(b"not found"),
        );
    };
    respond(StatusCode::OK, asset.mime, load(asset))
}

/// В debug-сборке файлы читаются с диска: правки интерфейса видны без пересборки.
#[cfg(debug_assertions)]
fn load(asset: &Asset) -> Cow<'static, [u8]> {
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/ui");
    std::fs::read(format!("{file}{}", asset.path))
        .map(Cow::Owned)
        .unwrap_or(Cow::Borrowed(asset.bytes))
}

#[cfg(not(debug_assertions))]
fn load(asset: &Asset) -> Cow<'static, [u8]> {
    Cow::Borrowed(asset.bytes)
}

fn respond(
    status: StatusCode,
    mime: &str,
    body: Cow<'static, [u8]>,
) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, mime)
        .header("Content-Security-Policy", CSP)
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", "no-store")
        .body(body)
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(b"")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_check() {
        assert!(is_ui_origin(&url()));
        assert!(!is_ui_origin("https://evil.example/axiom.localhost/"));
        assert!(!is_ui_origin("https://axiom.localhost.evil.example/"));
    }

    #[test]
    fn serves_known_files_only() {
        let req = |p: &str| {
            Request::builder()
                .uri(format!("{ORIGIN}{p}"))
                .body(Vec::new())
        };
        let ok = req("app.js").map(|r| serve(&r).status());
        assert_eq!(ok.ok(), Some(StatusCode::OK));
        let missing = req("../Cargo.toml").map(|r| serve(&r).status());
        assert_eq!(missing.ok(), Some(StatusCode::NOT_FOUND));
    }
}

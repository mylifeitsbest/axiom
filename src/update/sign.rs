//! Подпись обновлений: `SHA256SUMS.signed` рядом с `SHA256SUMS.txt`. Только Windows:
//! на Linux Axiom обновляет пакетный менеджер, а его репозитории подписаны своим ключом.
//!
//! Одна сумма из того же релиза защищает только от порчи при скачивании: кто
//! подменил exe в релизе, подменит и сумму. Поэтому суммы подписывает CI
//! (packaging/sign-sums.sh) ключом ECDSA P-256, закрытая часть которого есть
//! только в секрете GitHub Actions, а открытая зашита сюда, в [`KEYS`]. Без
//! верной подписи обновление не ставится.
//!
//! Формат файла: первая строка — подпись r‖s в hex, дальше подписанный текст:
//! `axiom-update 1`, `version <версия>` и строки `SHA256SUMS.txt`. Версия внутри
//! подписи не даёт выдать старую подписанную сборку (с уже исправленной дырой)
//! за новую. Проверяет подпись сама Windows (CNG, bcrypt.dll): своей криптографии
//! в Axiom нет.

use anyhow::{Result, anyhow, bail};

use super::Version;

/// Файл с подписанными суммами в релизе.
pub const FILE: &str = "SHA256SUMS.signed";

/// Открытые ключи подписи: точка P-256 (X‖Y). Несколько — чтобы сменить ключ,
/// выпустив сначала версию, которая знает и прежний, и новый.
/// Тот же ключ — в packaging/keys/update-signing.pem и install.ps1.
const KEYS: &[[u8; 64]] = &[[
    0x6b, 0x93, 0x76, 0xd1, 0x8b, 0x5d, 0xda, 0x65, 0xcd, 0x48, 0xa2, 0xee, 0xb7, 0x0f, 0xa5, 0x24,
    0xaa, 0x83, 0x26, 0x32, 0x2a, 0xb5, 0x34, 0xdc, 0xee, 0x8a, 0xed, 0x6a, 0x06, 0x99, 0xf0, 0xf0,
    0x9c, 0xde, 0x64, 0x87, 0x97, 0x57, 0xe8, 0xfb, 0xfe, 0x6d, 0x9d, 0xfc, 0x28, 0x36, 0x6a, 0x67,
    0x3d, 0xcf, 0x9f, 0x75, 0x32, 0xdd, 0x85, 0xaa, 0x94, 0x27, 0x88, 0x7f, 0x82, 0x4f, 0x94, 0x66,
]];

/// Первая строка подписанного текста: формат файла.
const HEADER: &str = "axiom-update 1";

/// Суммы из `file`, если подпись верна и подписана именно версия `expected`.
pub fn sums(file: &[u8], expected: Version) -> Result<String> {
    sums_with(file, expected, &keys())
}

/// Ключи, которым верим. В debug-сборке с локальным сервером обновлений
/// (`AXIOM_UPDATE_URL`) к ним добавляется тестовый `AXIOM_UPDATE_KEY` (hex X‖Y):
/// так обновление проверяют целиком, не трогая настоящий ключ.
fn keys() -> Vec<[u8; 64]> {
    #[allow(unused_mut)]
    let mut keys = KEYS.to_vec();
    #[cfg(debug_assertions)]
    if super::base_url().starts_with("http://127.0.0.1:")
        && let Some(key) = std::env::var("AXIOM_UPDATE_KEY")
            .ok()
            .and_then(|hex| from_hex::<64>(hex.trim()))
    {
        keys.push(key);
    }
    keys
}

fn sums_with(file: &[u8], expected: Version, keys: &[[u8; 64]]) -> Result<String> {
    let text = std::str::from_utf8(file).map_err(|_| anyhow!("{FILE} — не текст"))?;
    let (sig, body) = text
        .split_once('\n')
        .ok_or_else(|| anyhow!("{FILE} пустой или обрезан"))?;
    let sig = from_hex::<64>(sig.trim_end_matches('\r'))
        .ok_or_else(|| anyhow!("в {FILE} нет подписи"))?;
    let digest = sha256(body.as_bytes());
    if !keys.iter().any(|key| verify(key, &digest, &sig)) {
        bail!("подпись обновления не сошлась — файл подменён, обновление не ставим");
    }
    let mut lines = body.lines();
    if lines.next() != Some(HEADER) {
        bail!("{FILE}: незнакомый формат");
    }
    let signed = lines
        .next()
        .and_then(|line| line.strip_prefix("version "))
        .and_then(Version::parse)
        .ok_or_else(|| anyhow!("{FILE}: нет версии"))?;
    if signed != expected {
        bail!(
            "подписана версия {signed}, а не {expected} — сборка ещё обновляется, попробуйте позже"
        );
    }
    Ok(lines.collect::<Vec<_>>().join("\n"))
}

fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).into()
}

/// Ровно `N` байт из hex (без пробелов и префиксов).
fn from_hex<const N: usize>(hex: &str) -> Option<[u8; N]> {
    // `from_str_radix` пропустил бы и знак `+`.
    if hex.len() != N * 2 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; N];
    for (byte, pair) in out.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
        let pair = std::str::from_utf8(pair).ok()?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

/// ECDSA P-256: подпись `sig` (r‖s) хеша `digest` ключом `key` (X‖Y) — силами Windows.
fn verify(key: &[u8; 64], digest: &[u8; 32], sig: &[u8; 64]) -> bool {
    use windows::{
        Win32::Security::Cryptography::{
            BCRYPT_ALG_HANDLE, BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM,
            BCRYPT_ECDSA_PUBLIC_P256_MAGIC, BCRYPT_FLAGS, BCRYPT_KEY_HANDLE,
            BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCryptCloseAlgorithmProvider, BCryptDestroyKey,
            BCryptImportKeyPair, BCryptOpenAlgorithmProvider, BCryptVerifySignature,
        },
        core::PCWSTR,
    };

    // BCRYPT_ECCKEY_BLOB: магия открытого ключа P-256, длина координаты, затем X и Y.
    let mut blob = Vec::with_capacity(8 + key.len());
    blob.extend_from_slice(&BCRYPT_ECDSA_PUBLIC_P256_MAGIC.to_le_bytes());
    blob.extend_from_slice(&32u32.to_le_bytes());
    blob.extend_from_slice(key);
    // SAFETY: указатели — на живые локальные переменные и срезы; ключ и провайдер
    // закрываются здесь же, после последнего использования.
    unsafe {
        let mut alg = BCRYPT_ALG_HANDLE::default();
        if BCryptOpenAlgorithmProvider(
            &mut alg,
            BCRYPT_ECDSA_P256_ALGORITHM,
            PCWSTR::null(),
            BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
        )
        .is_err()
        {
            return false;
        }
        let mut handle = BCRYPT_KEY_HANDLE::default();
        let imported =
            BCryptImportKeyPair(alg, None, BCRYPT_ECCPUBLIC_BLOB, &mut handle, &blob, 0).is_ok();
        let ok =
            imported && BCryptVerifySignature(handle, None, digest, sig, BCRYPT_FLAGS(0)).is_ok();
        if imported {
            let _ = BCryptDestroyKey(handle);
        }
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        ok
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Тестовая пара (не настоящий ключ) и файл, подписанный packaging/sign-sums.sh.
    const TEST_KEY: &str = include_str!("testdata/key.hex");
    const SIGNED: &str = include_str!("testdata/SHA256SUMS.signed");

    fn test_key() -> [u8; 64] {
        from_hex::<64>(TEST_KEY.trim()).unwrap()
    }

    fn version() -> Version {
        Version::parse("0.6.2-beta.7").unwrap()
    }

    #[test]
    fn accepts_file_signed_by_the_script() {
        let sums = sums_with(SIGNED.as_bytes(), version(), &[test_key()]).unwrap();
        assert!(super::super::checksum_for(&sums, "Axiom-windows-x64.exe").is_some());
        assert!(!sums.contains("axiom-update"));
    }

    #[test]
    fn rejects_other_key_version_and_tampering() {
        // Настоящий ключ этот тестовый файл не подписывал.
        assert!(sums_with(SIGNED.as_bytes(), version(), KEYS).is_err());
        // Подписана другая версия.
        let older = Version::parse("0.6.2-beta.6").unwrap();
        let err = sums_with(SIGNED.as_bytes(), older, &[test_key()]).unwrap_err();
        assert!(err.to_string().contains("подписана версия"));
        // Подменили одну цифру суммы.
        let at = SIGNED.find("Axiom-windows-x64.exe").unwrap() - 3;
        let mut tampered = SIGNED.as_bytes().to_vec();
        tampered[at] = if tampered[at] == b'0' { b'1' } else { b'0' };
        let err = sums_with(&tampered, version(), &[test_key()]).unwrap_err();
        assert!(err.to_string().contains("не сошлась"));
        // Без подписи, пустой, мусор.
        let body = SIGNED.split_once('\n').unwrap().1;
        assert!(sums_with(body.as_bytes(), version(), &[test_key()]).is_err());
        assert!(sums_with(b"", version(), &[test_key()]).is_err());
        assert!(sums_with(b"\xff\xfe\n", version(), &[test_key()]).is_err());
    }

    #[test]
    fn real_key_is_a_valid_point() {
        // Windows принимает зашитый ключ: иначе обновление не прошло бы никогда.
        // Подпись нулями не сходится, но импорт ключа проходит.
        use windows::{
            Win32::Security::Cryptography::{
                BCRYPT_ALG_HANDLE, BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM,
                BCRYPT_ECDSA_PUBLIC_P256_MAGIC, BCRYPT_KEY_HANDLE,
                BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCryptCloseAlgorithmProvider,
                BCryptDestroyKey, BCryptImportKeyPair, BCryptOpenAlgorithmProvider,
            },
            core::PCWSTR,
        };
        for key in KEYS {
            let mut blob = BCRYPT_ECDSA_PUBLIC_P256_MAGIC.to_le_bytes().to_vec();
            blob.extend_from_slice(&32u32.to_le_bytes());
            blob.extend_from_slice(key);
            // SAFETY: как в `verify`.
            unsafe {
                let mut alg = BCRYPT_ALG_HANDLE::default();
                assert!(
                    BCryptOpenAlgorithmProvider(
                        &mut alg,
                        BCRYPT_ECDSA_P256_ALGORITHM,
                        PCWSTR::null(),
                        BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
                    )
                    .is_ok()
                );
                let mut handle = BCRYPT_KEY_HANDLE::default();
                let ok =
                    BCryptImportKeyPair(alg, None, BCRYPT_ECCPUBLIC_BLOB, &mut handle, &blob, 0)
                        .is_ok();
                if ok {
                    let _ = BCryptDestroyKey(handle);
                }
                let _ = BCryptCloseAlgorithmProvider(alg, 0);
                assert!(ok, "Windows не принимает ключ подписи");
            }
            assert!(!verify(key, &[0; 32], &[0; 64]));
        }
    }

    #[test]
    fn hex() {
        assert_eq!(from_hex::<2>("0aFf"), Some([0x0a, 0xff]));
        assert_eq!(from_hex::<2>("0aF"), None);
        assert_eq!(from_hex::<2>("0aFg"), None);
        assert_eq!(from_hex::<2>("+a0f"), None);
        assert_eq!(from_hex::<1>("яя"), None);
    }
}

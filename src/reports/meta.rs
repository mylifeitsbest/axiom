//! Метаданные фото и видео: место съёмки, камера, время — вырезаются до отправки.
//!
//! Сами кадры не перекодируются: JPEG и PNG теряют только служебные блоки, а у
//! MP4/MOV блоки с метаданными затираются в пустые `free` — размеры и смещения
//! кадров остаются прежними, видео играет как раньше.

/// JPEG без EXIF, XMP, IPTC, комментариев и вложенных кадров MPF (у телефонов в них
/// своя копия EXIF). Поворот из EXIF (Orientation) остаётся — иначе фото с телефона
/// легло бы набок. Если файл не разобрать, `None`: тогда его лучше не отправлять как есть.
pub fn jpeg(data: &[u8]) -> Option<Vec<u8>> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&[0xFF, 0xD8]);
    // Новый EXIF с одним поворотом встаёт сразу после SOI (или после JFIF).
    let mut exif_at = out.len();
    let mut orientation = None;
    let mut i = 2;
    loop {
        if *data.get(i)? != 0xFF {
            return None;
        }
        // Перед маркером может быть сколько угодно байтов заполнения FF.
        while data.get(i) == Some(&0xFF) {
            i += 1;
        }
        let marker = *data.get(i)?;
        i += 1;
        match marker {
            // EOI: всё, что дальше (вложенные кадры, хвосты камер), не нужно.
            0xD9 => {
                out.extend_from_slice(&[0xFF, 0xD9]);
                break;
            }
            0x01 | 0xD0..=0xD7 => {
                out.extend_from_slice(&[0xFF, marker]);
                continue;
            }
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*data.get(i)?, *data.get(i + 1)?]));
        let segment = data.get(i..i.checked_add(len)?)?;
        let body = segment.get(2..)?;
        let drop = match marker {
            // APP1: EXIF и XMP (и расширенный XMP).
            0xE1 => {
                if let Some(tiff) = body.strip_prefix(b"Exif\0\0") {
                    orientation = orientation.or_else(|| exif_orientation(tiff));
                }
                true
            }
            // APP2: профиль цвета нужен, оглавление MPF — нет (вложенных кадров не будет).
            0xE2 => body.starts_with(b"MPF\0"),
            // APP14 Adobe говорит, как переводить цвета, — без него CMYK-фото испортится.
            0xEE => !body.starts_with(b"Adobe"),
            // Прочие APP3…APP15 (IPTC, данные камер) и комментарии.
            0xE3..=0xEF | 0xFE => true,
            _ => false,
        };
        if !drop {
            out.extend_from_slice(&[0xFF, marker]);
            out.extend_from_slice(segment);
            if marker == 0xE0 && exif_at == 2 {
                exif_at = out.len();
            }
        }
        i += len;
        if marker == 0xDA {
            // Сжатые данные кадра — до следующего маркера (FF 00 и RSTn — их часть).
            let start = i;
            loop {
                let ff = i + data.get(i..)?.iter().position(|&b| b == 0xFF)?;
                match *data.get(ff + 1)? {
                    0x00 | 0xD0..=0xD7 => i = ff + 2,
                    0xFF => i = ff + 1,
                    _ => {
                        out.extend_from_slice(&data[start..ff]);
                        i = ff;
                        break;
                    }
                }
            }
        }
    }
    if let Some(value) = orientation.filter(|&v| v != 1) {
        out.splice(exif_at..exif_at, orientation_exif(value));
    }
    Some(out)
}

/// Orientation (тег 0x0112) из первого каталога EXIF.
fn exif_orientation(tiff: &[u8]) -> Option<u16> {
    let little = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| {
        let b = [*tiff.get(at)?, *tiff.get(at + 1)?];
        Some(if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |at: usize| {
        let b: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    for k in 0..usize::from(u16_at(ifd)?) {
        let entry = ifd + 2 + 12 * k;
        // Тип 3 — SHORT, значение лежит прямо в записи.
        if u16_at(entry)? == 0x0112 && u16_at(entry + 2)? == 3 {
            return u16_at(entry + 8).filter(|v| (1..=8).contains(v));
        }
    }
    None
}

/// Сегмент APP1 с EXIF из одного тега Orientation.
fn orientation_exif(value: u16) -> Vec<u8> {
    let mut seg = vec![0xFF, 0xE1, 0, 34];
    seg.extend_from_slice(b"Exif\0\0MM\0\x2a\0\0\0\x08");
    // Один тег: 0x0112, SHORT, 1 значение, следующего каталога нет.
    seg.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1]);
    seg.extend_from_slice(&value.to_be_bytes());
    seg.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    seg
}

/// PNG без текстовых блоков, EXIF и времени создания.
pub fn png(data: &[u8]) -> Option<Vec<u8>> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !data.starts_with(SIGNATURE) {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(SIGNATURE);
    let mut i = SIGNATURE.len();
    while i < data.len() {
        let len = u32::from_be_bytes(data.get(i..i + 4)?.try_into().ok()?);
        let end = i.checked_add(12)?.checked_add(usize::try_from(len).ok()?)?;
        let chunk = data.get(i..end)?;
        let kind = &chunk[4..8];
        if !matches!(kind, b"eXIf" | b"tEXt" | b"zTXt" | b"iTXt" | b"tIME") {
            out.extend_from_slice(chunk);
        }
        i = end;
        if kind == b"IEND" {
            break;
        }
    }
    Some(out)
}

/// XMP в MP4 лежит в `uuid` с этим номером.
const XMP_UUID: [u8; 16] = [
    0xBE, 0x7A, 0xCF, 0xCB, 0x97, 0xA9, 0x42, 0xE8, 0x9C, 0x71, 0x99, 0x94, 0x91, 0xE3, 0xAF, 0xAC,
];

/// MP4, MOV, 3GP: блоки `udta` и `meta` (координаты, модель телефона, время) в файле,
/// `moov` и дорожках и XMP становятся пустыми `free` того же размера — проигрыватели
/// такие пропускают.
pub fn bmff(data: &mut [u8]) {
    walk(data, 0, data.len(), 0);
}

fn walk(data: &mut [u8], mut i: usize, end: usize, depth: u8) {
    while i + 8 <= end {
        let Ok(head) = <[u8; 4]>::try_from(&data[i..i + 4]) else {
            return;
        };
        let (header, size) = match u32::from_be_bytes(head) {
            // Размер в 64 битах сразу за типом.
            1 => match data
                .get(i + 8..i + 16)
                .and_then(|b| <[u8; 8]>::try_from(b).ok())
            {
                Some(b) => (
                    16,
                    usize::try_from(u64::from_be_bytes(b)).unwrap_or(usize::MAX),
                ),
                None => return,
            },
            // До конца родителя.
            0 => (8, end - i),
            n => (8, usize::try_from(n).unwrap_or(usize::MAX)),
        };
        let Some(box_end) = i.checked_add(size).filter(|&e| size >= header && e <= end) else {
            return;
        };
        let kind = [data[i + 4], data[i + 5], data[i + 6], data[i + 7]];
        let xmp = data.get(i + header..i + header + 16) == Some(&XMP_UUID[..]);
        let wipe = match &kind {
            b"udta" | b"meta" => true,
            b"uuid" => xmp,
            b"moov" | b"trak" if depth < 3 => {
                walk(data, i + header, box_end, depth + 1);
                false
            }
            _ => false,
        };
        if wipe {
            // Одного переименования мало: координаты остались бы в байтах файла.
            data[i + 4..i + 8].copy_from_slice(b"free");
            data[i + header..box_end].fill(0);
        }
        i = box_end;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn segment(marker: u8, body: &[u8]) -> Vec<u8> {
        let len = u16::try_from(body.len() + 2).unwrap();
        let mut s = vec![0xFF, marker];
        s.extend_from_slice(&len.to_be_bytes());
        s.extend_from_slice(body);
        s
    }

    /// EXIF (little endian) с поворотом и ссылкой на GPS.
    fn exif(orientation: u16) -> Vec<u8> {
        let mut t = b"Exif\0\0II\x2a\0\x08\0\0\0".to_vec();
        t.extend_from_slice(&2u16.to_le_bytes());
        t.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0]);
        t.extend_from_slice(&orientation.to_le_bytes());
        t.extend_from_slice(&[0, 0]);
        t.extend_from_slice(&[0x25, 0x88, 4, 0, 1, 0, 0, 0, 0x26, 0, 0, 0]);
        t.extend_from_slice(&[0, 0, 0, 0]);
        t.extend_from_slice(b"GPS 55.75N 37.62E");
        t
    }

    fn photo(orientation: u16) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        j.extend(segment(0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"));
        j.extend(segment(0xE1, &exif(orientation)));
        j.extend(segment(0xE2, b"ICC_PROFILE\0\x01\x01icc"));
        j.extend(segment(0xE2, b"MPF\0offsets"));
        j.extend(segment(0xED, b"Photoshop 3.0\0IPTC city"));
        j.extend(segment(0xFE, b"comment"));
        j.extend(segment(0xDB, &[0; 65]));
        j.extend(segment(0xDA, &[1, 1, 0, 0, 63, 0]));
        // Сжатые данные с FF 00 и RST0 внутри.
        j.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]);
        j.extend_from_slice(&[0xFF, 0xD9]);
        // Вложенный кадр MPF со своим EXIF.
        j.extend_from_slice(&[0xFF, 0xD8]);
        j.extend(segment(0xE1, &exif(1)));
        j.extend_from_slice(&[0xFF, 0xD9]);
        j
    }

    fn has(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn jpeg_loses_exif_but_keeps_rotation_and_image() {
        let out = jpeg(&photo(6)).unwrap();
        assert!(!has(&out, b"GPS"), "место съёмки осталось");
        assert!(!has(&out, b"IPTC") && !has(&out, b"comment") && !has(&out, b"MPF\0"));
        assert!(has(&out, b"ICC_PROFILE") && has(&out, b"JFIF"));
        assert!(has(
            &out,
            &[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56, 0xFF, 0xD9]
        ));
        // Один SOI и один EOI: вложенного кадра нет.
        assert!(out.ends_with(&[0xFF, 0xD9]));
        assert_eq!(out.windows(2).filter(|w| w == &[0xFF, 0xD8]).count(), 1);
        // Новый EXIF сразу после JFIF (SOI, маркер, длина, 14 байт), и в нём поворот 6.
        let app0 = 2 + 4 + 14;
        assert_eq!(&out[app0..app0 + 2], &[0xFF, 0xE1]);
        assert_eq!(exif_orientation(&out[app0 + 10..]), Some(6));
    }

    #[test]
    fn jpeg_without_rotation_gets_no_exif() {
        let out = jpeg(&photo(1)).unwrap();
        assert!(!has(&out, b"Exif"));
        // Повторный проход ничего не меняет.
        assert_eq!(jpeg(&out).unwrap(), out);
        assert_eq!(jpeg(b"not a jpeg"), None);
        assert_eq!(jpeg(&[0xFF, 0xD8, 0xFF, 0xE1, 0x00]), None);
    }

    #[test]
    fn png_loses_text_chunks() {
        let chunk = |kind: &[u8], body: &[u8]| {
            let mut c = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
            c.extend_from_slice(kind);
            c.extend_from_slice(body);
            c.extend_from_slice(&[0; 4]);
            c
        };
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        p.extend(chunk(b"IHDR", &[0; 13]));
        p.extend(chunk(b"tEXt", b"Author\0Roman"));
        p.extend(chunk(b"eXIf", b"MM\0*GPS"));
        p.extend(chunk(b"IDAT", b"pixels"));
        p.extend(chunk(b"IEND", b""));
        let out = png(&p).unwrap();
        assert!(!has(&out, b"Roman") && !has(&out, b"GPS"));
        assert!(
            has(&out, b"IHDR")
                && has(&out, b"pixels")
                && out.ends_with(&[b'I', b'E', b'N', b'D', 0, 0, 0, 0])
        );
        assert_eq!(png(b"GIF89a"), None);
    }

    #[test]
    fn video_metadata_boxes_become_free() {
        let boxed = |kind: &[u8], body: &[u8]| {
            let mut b = u32::try_from(body.len() + 8)
                .unwrap()
                .to_be_bytes()
                .to_vec();
            b.extend_from_slice(kind);
            b.extend_from_slice(body);
            b
        };
        let trak = boxed(
            b"trak",
            &[boxed(b"tkhd", &[0; 4]), boxed(b"udta", b"model")].concat(),
        );
        let moov = boxed(
            b"moov",
            &[
                boxed(b"mvhd", &[0; 4]),
                boxed(b"udta", b"\xa9xyz+55.75+037.62/"),
                boxed(b"meta", b"keys"),
                trak,
            ]
            .concat(),
        );
        let mut xmp = XMP_UUID.to_vec();
        xmp.extend_from_slice(b"<x:xmp/>");
        let mut v = [
            boxed(b"ftyp", b"isom"),
            moov,
            boxed(b"uuid", &xmp),
            boxed(b"mdat", b"frames"),
        ]
        .concat();
        let size = v.len();
        bmff(&mut v);
        assert_eq!(v.len(), size);
        assert!(!has(&v, b"udta") && !has(&v, b"meta") && !has(&v, b"uuid"));
        // Содержимое затёрто, а не только переименовано.
        assert!(!has(&v, b"model") && !has(&v, b"+55.75") && !has(&v, b"keys") && !has(&v, b"xmp"));
        assert_eq!(v.windows(4).filter(|w| w == b"free").count(), 4);
        assert!(has(&v, b"moov") && has(&v, b"trak") && has(&v, b"mdat") && has(&v, b"frames"));
        // Битый размер — ничего не трогаем дальше.
        let mut broken = b"\0\0\0\x04udta".to_vec();
        bmff(&mut broken);
        assert_eq!(&broken[4..], b"udta");
    }
}

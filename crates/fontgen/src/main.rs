//! GNU Unifont の `.hex` 形式を読み、firmware に埋め込むビットマップ配列を生成する。
//!
//! `.hex` の各行は `<code point 4-6 桁>:<glyph の 16 進>` であり、glyph は 16 行 x 8 列
//! (32 桁) または 16 行 x 16 列 (64 桁) のいずれかである。
//!
//! 標準入力を検証し、選択した文字を code point 順の固定長レコードとして出力する。

use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
};

/// 1 文字分の glyph。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyph {
    pub code_point: u32,
    /// 横幅 (8 または 16)。
    pub width: u8,
    /// 行ごとのビット列。上から順に 16 行。幅 8 では 1 行 1 バイト、幅 16 では 2 バイト。
    pub rows: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingSeparator,
    BadCodePoint,
    BadLength(usize),
    BadHex,
    Duplicate,
}

/// `.hex` の 1 行を解析する。
pub fn parse_line(line: &str) -> Result<Glyph, ParseError> {
    let (cp, hex) = line
        .trim_end()
        .split_once(':')
        .ok_or(ParseError::MissingSeparator)?;
    let code_point = u32::from_str_radix(cp, 16).map_err(|_| ParseError::BadCodePoint)?;
    if char::from_u32(code_point).is_none() {
        return Err(ParseError::BadCodePoint);
    }
    if !hex.is_ascii() {
        return Err(ParseError::BadHex);
    }
    let width = match hex.len() {
        32 => 8,
        64 => 16,
        n => return Err(ParseError::BadLength(n)),
    };
    let rows = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| ParseError::BadHex))
        .collect::<Result<Vec<u8>, _>>()?;
    Ok(Glyph {
        code_point,
        width,
        rows,
    })
}

fn selected(cp: u32) -> bool {
    matches!(cp, 0x20..=0x7e | 0x2000..=0x206f | 0x25a1 | 0x3000..=0x30ff |
        0x4e00..=0x9fff | 0xff00..=0xffef)
}

fn generate(input: impl BufRead) -> io::Result<Vec<u8>> {
    let mut glyphs = BTreeMap::new();
    for (index, line) in input.lines().enumerate() {
        let glyph = parse_line(&line?).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line {}: {error:?}", index + 1),
            )
        })?;
        if selected(glyph.code_point) && glyphs.insert(glyph.code_point, glyph).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate code point",
            ));
        }
    }
    if !glyphs.contains_key(&0x25a1) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing fallback glyph",
        ));
    }
    let mut output = Vec::new();
    for glyph in glyphs.values() {
        output.extend_from_slice(&glyph.code_point.to_le_bytes());
        output.push(glyph.width);
        // レコードは 37 byte。幅にかかわらず各行を big endian の 16 bit とする。
        for row in glyph.rows.chunks(usize::from(glyph.width / 8)) {
            output.push(row[0]);
            output.push(if glyph.width == 16 { row[1] } else { 0 });
        }
    }
    Ok(output)
}

fn main() -> io::Result<()> {
    let output = generate(io::stdin().lock())?;
    io::stdout().lock().write_all(&output)?;
    eprintln!("glyphs: {}, bytes: {}", output.len() / 37, output.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_half_width_glyph() {
        let g = parse_line("0041:0000000018242442427E424242420000").unwrap();
        assert_eq!(g.code_point, 0x41);
        assert_eq!(g.width, 8);
        assert_eq!(g.rows.len(), 16);
        assert_eq!(g.rows[4], 0x18);
    }

    #[test]
    fn parses_full_width_glyph() {
        let hex = "3042:".to_string() + &"00".repeat(32);
        let g = parse_line(&hex).unwrap();
        assert_eq!(g.width, 16);
        assert_eq!(g.rows.len(), 32);
    }

    #[test]
    fn rejects_bad_length() {
        assert_eq!(parse_line("0041:00"), Err(ParseError::BadLength(2)));
    }

    #[test]
    fn malformed_unicode_does_not_panic() {
        assert_eq!(
            parse_line(&format!("0041:{}", "日".repeat(32))),
            Err(ParseError::BadHex)
        );
        assert_eq!(
            parse_line(&format!("D800:{}", "00".repeat(16))),
            Err(ParseError::BadCodePoint)
        );
    }

    #[test]
    fn output_is_sorted_and_rejects_duplicates() {
        let input = format!("25A1:{}\n0041:{}\n", "ff".repeat(32), "18".repeat(16));
        let bytes = generate(input.as_bytes()).unwrap();
        assert_eq!(bytes.len(), 74);
        assert_eq!(&bytes[..7], &[65, 0, 0, 0, 8, 0x18, 0]);
        assert!(generate(format!("{input}{input}").as_bytes()).is_err());
    }
}

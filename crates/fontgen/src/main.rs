//! GNU Unifont の `.hex` 形式を読み、firmware に埋め込むビットマップ配列を生成する。
//!
//! `.hex` の各行は `<code point 4-6 桁>:<glyph の 16 進>` であり、glyph は 16 行 x 8 列
//! (32 桁) または 16 行 x 16 列 (64 桁) のいずれかである。
//!
//! 現段階では解析のみを実装する。収録範囲 (ASCII + JIS X 0208 の部分集合) の選定と
//! 出力形式は docs/design.md のフォントの節で確定してから実装する。

use std::io::{self, BufRead};

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
}

/// `.hex` の 1 行を解析する。
pub fn parse_line(line: &str) -> Result<Glyph, ParseError> {
    let (cp, hex) = line
        .trim_end()
        .split_once(':')
        .ok_or(ParseError::MissingSeparator)?;
    let code_point = u32::from_str_radix(cp, 16).map_err(|_| ParseError::BadCodePoint)?;
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

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut count = 0usize;
    let mut wide = 0usize;
    for line in stdin.lock().lines() {
        let line = line?;
        match parse_line(&line) {
            Ok(g) => {
                count += 1;
                if g.width == 16 {
                    wide += 1;
                }
            }
            Err(e) => eprintln!("skip: {e:?}: {line}"),
        }
    }
    println!("glyphs: {count} (wide: {wide})");
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
}

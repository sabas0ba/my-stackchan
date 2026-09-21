//! 24-bit BMP の指定領域を、Card 用の小さな RGB565 画像へ変換する。

use std::{fs, path::Path};

use protocol::{ImageData, MAX_IMAGE_SIDE};

const MAX_BMP_FILE_BYTES: u64 = 16 * 1024 * 1024;

pub fn load_icon(path: &Path, x: u32, y: u32, width: u8, height: u8) -> Result<ImageData, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("BMP を開けません: {error}"))?;
    if metadata.len() > MAX_BMP_FILE_BYTES {
        return Err("BMP は 16 MiB 以下にしてください".into());
    }
    let bytes = fs::read(path).map_err(|error| format!("BMP を読めません: {error}"))?;
    decode_icon(&bytes, x, y, width, height)
}

fn decode_icon(bytes: &[u8], x: u32, y: u32, width: u8, height: u8) -> Result<ImageData, String> {
    if !(1..=MAX_IMAGE_SIDE).contains(&width) || !(1..=MAX_IMAGE_SIDE).contains(&height) {
        return Err("画像サイズは縦横とも 1..16 px にしてください".into());
    }
    if bytes.len() < 54 || &bytes[..2] != b"BM" {
        return Err("24-bit BMP のヘッダがありません".into());
    }
    let read_u16 =
        |offset: usize| u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
    let read_u32 =
        |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let read_i32 =
        |offset: usize| i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let file_size = usize::try_from(read_u32(2)).map_err(|_| "BMP の長さが不正です")?;
    let data_offset = usize::try_from(read_u32(10)).map_err(|_| "BMP の位置が不正です")?;
    if file_size != bytes.len()
        || read_u32(14) != 40
        || read_u16(26) != 1
        || read_u16(28) != 24
        || read_u32(30) != 0
        || data_offset < 54
    {
        return Err("無圧縮の 24-bit BMP (BITMAPINFOHEADER) が必要です".into());
    }
    let bmp_width = usize::try_from(read_i32(18)).map_err(|_| "BMP の幅が不正です")?;
    let signed_height = read_i32(22);
    let bmp_height =
        usize::try_from(signed_height.unsigned_abs()).map_err(|_| "BMP の高さが不正です")?;
    if bmp_width == 0 || bmp_height == 0 || signed_height == i32::MIN {
        return Err("BMP の幅・高さが不正です".into());
    }
    let stride = bmp_width
        .checked_mul(3)
        .and_then(|value| value.checked_add(3))
        .map(|value| value & !3)
        .ok_or("BMP の幅が大きすぎます")?;
    let end = stride
        .checked_mul(bmp_height)
        .and_then(|value| data_offset.checked_add(value))
        .ok_or("BMP のサイズが大きすぎます")?;
    if end > bytes.len() {
        return Err("BMP の画素データが不足しています".into());
    }
    let x = usize::try_from(x).map_err(|_| "画像の x 座標が大きすぎます")?;
    let y = usize::try_from(y).map_err(|_| "画像の y 座標が大きすぎます")?;
    if x.checked_add(usize::from(width))
        .is_none_or(|value| value > bmp_width)
        || y.checked_add(usize::from(height))
            .is_none_or(|value| value > bmp_height)
    {
        return Err("切り出す画像が BMP の範囲外です".into());
    }

    let mut icon = ImageData {
        width,
        height,
        pixels: Default::default(),
    };
    for row in y..y + usize::from(height) {
        let source_row = if signed_height < 0 {
            row
        } else {
            bmp_height - 1 - row
        };
        for column in x..x + usize::from(width) {
            let offset = data_offset + source_row * stride + column * 3;
            let blue = bytes[offset];
            let green = bytes[offset + 1];
            let red = bytes[offset + 2];
            let rgb565 =
                (u16::from(red >> 3) << 11) | (u16::from(green >> 2) << 5) | u16::from(blue >> 3);
            for byte in rgb565.to_be_bytes() {
                icon.pixels
                    .push(byte)
                    .map_err(|_| "画像のデータが大きすぎます")?;
            }
        }
    }
    Ok(icon)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bmp_2x2(top_down: bool) -> Vec<u8> {
        let mut bytes = vec![0; 54];
        bytes[..2].copy_from_slice(b"BM");
        bytes[2..6].copy_from_slice(&70u32.to_le_bytes());
        bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
        bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
        bytes[18..22].copy_from_slice(&2i32.to_le_bytes());
        bytes[22..26].copy_from_slice(&(if top_down { -2i32 } else { 2i32 }).to_le_bytes());
        bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
        let top = [0, 0, 255, 0, 255, 0, 0, 0];
        let bottom = [255, 0, 0, 255, 255, 255, 0, 0];
        if top_down {
            bytes.extend(top);
            bytes.extend(bottom);
        } else {
            bytes.extend(bottom);
            bytes.extend(top);
        }
        bytes
    }

    #[test]
    fn decodes_top_down_and_bottom_up_and_crops() {
        for top_down in [true, false] {
            let bmp = bmp_2x2(top_down);
            let icon = decode_icon(&bmp, 0, 0, 2, 2).unwrap();
            assert_eq!(
                icon.pixels.as_slice(),
                &[0xF8, 0, 0x07, 0xE0, 0, 0x1F, 0xFF, 0xFF]
            );
            let cropped = decode_icon(&bmp, 1, 0, 1, 1).unwrap();
            assert_eq!(cropped.pixels.as_slice(), &[0x07, 0xE0]);
        }
    }

    #[test]
    fn rejects_truncated_unsupported_and_out_of_bounds_bmp() {
        let mut bmp = bmp_2x2(true);
        assert!(decode_icon(&bmp[..69], 0, 0, 2, 2).is_err());
        assert!(decode_icon(&bmp, 1, 0, 2, 2).is_err());
        assert!(decode_icon(&bmp, 0, 0, 17, 1).is_err());
        bmp[28] = 32;
        assert!(decode_icon(&bmp, 0, 0, 2, 2).is_err());
    }
}

//! 画像のデコード（#36）。PNG は `png`、JPEG は `zune-jpeg` で RGBA8 にする。
//!
//! GPU には触れない（Lua の壁紙は EGL surface より先に読み込むことがあるため）。
//! テクスチャーにするのは `texture.rs`。

use std::io::Cursor;
use std::path::Path;

/// デコードした画像。`rgba` は上の行から順に並ぶ。
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// 上下を反転する（シェーダーの `vflip`）。
    pub fn flip_rows(&mut self) {
        let row = self.width as usize * 4;
        let h = self.height as usize;
        for y in 0..h / 2 {
            let (top, bottom) = self.rgba.split_at_mut((h - 1 - y) * row);
            top[y * row..(y + 1) * row].swap_with_slice(&mut bottom[..row]);
        }
    }
}

/// ファイルを読んでデコードする。縦横のどちらかが `max_size` を超えたら、画素をデコードする前にエラーにする。
pub fn load(path: &Path, max_size: u32) -> Result<Image, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode(&bytes, max_size).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn decode(bytes: &[u8], max_size: u32) -> Result<Image, String> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png(bytes, max_size)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        decode_jpeg(bytes, max_size)
    } else {
        Err("unsupported image format (PNG and JPEG only)".into())
    }
}

fn check_size(width: u32, height: u32, max_size: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err(format!("empty image ({width}x{height})"));
    }
    if width > max_size || height > max_size {
        return Err(format!(
            "image is too large ({width}x{height}, max {max_size}x{max_size} on this GPU)"
        ));
    }
    Ok(())
}

fn decode_png(bytes: &[u8], max_size: u32) -> Result<Image, String> {
    let mut d = png::Decoder::new(Cursor::new(bytes));
    // 大きさは下で GPU の上限と比べるので、png の既定（64MiB）では制限しない。
    d.set_limits(png::Limits { bytes: usize::MAX });
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = d.read_info().map_err(|e| format!("PNG: {e}"))?;
    let (width, height) = (r.info().width, r.info().height);
    check_size(width, height, max_size)?;
    let size = r.output_buffer_size().ok_or("PNG: image is too large")?;
    let mut buf = vec![0; size];
    let info = r.next_frame(&mut buf).map_err(|e| format!("PNG: {e}"))?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => expand(&buf, 3, |p| [p[0], p[1], p[2], 255]),
        png::ColorType::GrayscaleAlpha => expand(&buf, 2, |p| [p[0], p[0], p[0], p[1]]),
        png::ColorType::Grayscale => expand(&buf, 1, |p| [p[0], p[0], p[0], 255]),
        // EXPAND でパレットは RGB(A) に広がる。
        png::ColorType::Indexed => return Err("PNG: unexpected indexed output".into()),
    };
    Ok(Image {
        width,
        height,
        rgba,
    })
}

fn expand(src: &[u8], n: usize, f: impl Fn(&[u8]) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() / n * 4);
    for p in src.chunks_exact(n) {
        out.extend_from_slice(&f(p));
    }
    out
}

fn decode_jpeg(bytes: &[u8], max_size: u32) -> Result<Image, String> {
    use zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(max_size as usize)
        .set_max_height(max_size as usize);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    d.decode_headers().map_err(|e| format!("JPEG: {e}"))?;
    let (w, h) = d.dimensions().ok_or("JPEG: no dimensions")?;
    check_size(w as u32, h as u32, max_size)?;
    let orientation = d.exif().and_then(|e| exif_orientation(e)).unwrap_or(1);
    let rgba = d.decode().map_err(|e| format!("JPEG: {e}"))?;
    if rgba.len() != w * h * 4 {
        return Err("JPEG: unexpected output size".into());
    }
    Ok(orient(
        Image {
            width: w as u32,
            height: h as u32,
            rgba,
        },
        orientation,
    ))
}

/// EXIF の Orientation（1〜8）。読めなければ None。
/// `data` は TIFF のヘッダーから始まる（先頭に `Exif\0\0` があれば飛ばす）。
fn exif_orientation(data: &[u8]) -> Option<u16> {
    let tiff = data.strip_prefix(b"Exif\0\0").unwrap_or(data);
    let le = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b: [u8; 2] = tiff.get(o..o + 2)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b: [u8; 4] = tiff.get(o..o + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    for i in 0..count {
        let e = ifd + 2 + i * 12;
        // Orientation（0x0112）、型は SHORT（3）。値はエントリーの値の欄の先頭にある。
        if u16_at(e)? == 0x0112 && u16_at(e + 2)? == 3 {
            let v = u16_at(e + 8)?;
            return (1..=8).contains(&v).then_some(v);
        }
    }
    None
}

/// EXIF の Orientation に従って、画像ビューアーと同じ向きに並べ直す。
fn orient(img: Image, orientation: u16) -> Image {
    if orientation == 1 {
        return img;
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let swap = orientation >= 5;
    let (ow, oh) = if swap { (h, w) } else { (w, h) };
    let mut out = vec![0u8; img.rgba.len()];
    for y in 0..oh {
        for x in 0..ow {
            // 並べ直した後の (x, y) に来る、元の画像の画素。
            let (sx, sy) = match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                _ => (w - 1 - y, x),
            };
            let s = (sy * w + sx) * 4;
            let d = (y * ow + x) * 4;
            out[d..d + 4].copy_from_slice(&img.rgba[s..s + 4]);
        }
    }
    Image {
        width: ow as u32,
        height: oh as u32,
        rgba: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 画素ごとに番号を振った w×h の画像（R に番号を入れる）。
    fn numbered(w: u32, h: u32) -> Image {
        let rgba = (0..w * h).flat_map(|i| [i as u8, 0, 0, 255]).collect();
        Image {
            width: w,
            height: h,
            rgba,
        }
    }

    fn reds(img: &Image) -> Vec<u8> {
        img.rgba.chunks_exact(4).map(|p| p[0]).collect()
    }

    #[test]
    fn flip_rows_reverses_rows() {
        let mut img = numbered(2, 3);
        img.flip_rows();
        assert_eq!(reds(&img), [4, 5, 2, 3, 0, 1]);
    }

    #[test]
    fn orientations_match_their_definitions() {
        // 元の画像（3×2）:
        //   0 1 2
        //   3 4 5
        let img = numbered(3, 2);
        let cases: [(u16, (u32, u32), [u8; 6]); 8] = [
            (1, (3, 2), [0, 1, 2, 3, 4, 5]),
            // 左右反転
            (2, (3, 2), [2, 1, 0, 5, 4, 3]),
            // 180 度回転
            (3, (3, 2), [5, 4, 3, 2, 1, 0]),
            // 上下反転
            (4, (3, 2), [3, 4, 5, 0, 1, 2]),
            // 転置（左上と右下を結ぶ線で反転）
            (5, (2, 3), [0, 3, 1, 4, 2, 5]),
            // 時計回りに 90 度回転
            (6, (2, 3), [3, 0, 4, 1, 5, 2]),
            // 右上と左下を結ぶ線で反転
            (7, (2, 3), [5, 2, 4, 1, 3, 0]),
            // 反時計回りに 90 度回転
            (8, (2, 3), [2, 5, 1, 4, 0, 3]),
        ];
        for (o, size, want) in cases {
            let out = orient(img.clone(), o);
            assert_eq!((out.width, out.height), size, "orientation {o}");
            assert_eq!(reds(&out), want, "orientation {o}");
        }
    }

    /// Orientation のエントリーを 1 つ持つ TIFF。
    fn tiff(le: bool, value: u16) -> Vec<u8> {
        let u16b = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let u32b = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let mut t = Vec::new();
        t.extend_from_slice(if le { b"II" } else { b"MM" });
        t.extend_from_slice(&u16b(42));
        t.extend_from_slice(&u32b(8));
        t.extend_from_slice(&u16b(2));
        // 関係のないエントリー（ImageWidth）
        t.extend_from_slice(&u16b(0x0100));
        t.extend_from_slice(&u16b(3));
        t.extend_from_slice(&u32b(1));
        t.extend_from_slice(&u32b(0));
        t.extend_from_slice(&u16b(0x0112));
        t.extend_from_slice(&u16b(3));
        t.extend_from_slice(&u32b(1));
        t.extend_from_slice(&u16b(value));
        t.extend_from_slice(&[0, 0]);
        t
    }

    #[test]
    fn reads_exif_orientation_in_both_byte_orders() {
        assert_eq!(exif_orientation(&tiff(true, 6)), Some(6));
        assert_eq!(exif_orientation(&tiff(false, 8)), Some(8));
        let mut with_header = b"Exif\0\0".to_vec();
        with_header.extend(tiff(true, 3));
        assert_eq!(exif_orientation(&with_header), Some(3));
        assert_eq!(exif_orientation(&tiff(true, 9)), None);
        assert_eq!(exif_orientation(b"II*\0"), None);
        assert_eq!(exif_orientation(b""), None);
    }

    fn png_bytes(
        w: u32,
        h: u32,
        color: png::ColorType,
        depth: png::BitDepth,
        data: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(color);
        e.set_depth(depth);
        e.write_header().unwrap().write_image_data(data).unwrap();
        out
    }

    #[test]
    fn png_color_types_become_rgba() {
        let rgb = png_bytes(
            2,
            1,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &[1, 2, 3, 4, 5, 6],
        );
        assert_eq!(decode(&rgb, 16).unwrap().rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
        let ga = png_bytes(
            1,
            1,
            png::ColorType::GrayscaleAlpha,
            png::BitDepth::Eight,
            &[7, 8],
        );
        assert_eq!(decode(&ga, 16).unwrap().rgba, [7, 7, 7, 8]);
        let g = png_bytes(1, 1, png::ColorType::Grayscale, png::BitDepth::Eight, &[9]);
        assert_eq!(decode(&g, 16).unwrap().rgba, [9, 9, 9, 255]);
        // 16 ビットは上位 8 ビットにする。
        let g16 = png_bytes(
            1,
            1,
            png::ColorType::Grayscale,
            png::BitDepth::Sixteen,
            &[0xAB, 0xCD],
        );
        assert_eq!(decode(&g16, 16).unwrap().rgba, [0xAB, 0xAB, 0xAB, 255]);
        let rgba = png_bytes(
            1,
            2,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &[1, 2, 3, 4, 5, 6, 7, 8],
        );
        let img = decode(&rgba, 16).unwrap();
        assert_eq!((img.width, img.height), (1, 2));
        assert_eq!(img.rgba, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn png_palette_becomes_rgba() {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, 2, 1);
        e.set_color(png::ColorType::Indexed);
        e.set_depth(png::BitDepth::Eight);
        e.set_palette(vec![10, 20, 30, 40, 50, 60]);
        e.set_trns(vec![128]);
        e.write_header().unwrap().write_image_data(&[0, 1]).unwrap();
        assert_eq!(
            decode(&out, 16).unwrap().rgba,
            [10, 20, 30, 128, 40, 50, 60, 255]
        );
    }

    #[test]
    fn too_large_or_unknown_images_are_errors() {
        let rgb = png_bytes(3, 1, png::ColorType::Rgb, png::BitDepth::Eight, &[0; 9]);
        let e = decode(&rgb, 2).unwrap_err();
        assert!(e.contains("too large (3x1, max 2x2"), "{e}");
        assert!(
            decode(b"GIF89a", 16)
                .unwrap_err()
                .contains("PNG and JPEG only")
        );
        // 途中で切れたファイル
        assert!(decode(&rgb[..rgb.len() - 20], 16).is_err());
        assert!(decode(&[0xFF, 0xD8, 0xFF], 16).is_err());
    }

    #[test]
    fn missing_file_names_the_path() {
        let e = load(Path::new("/nonexistent/kagegami.png"), 16).unwrap_err();
        assert!(e.starts_with("/nonexistent/kagegami.png:"), "{e}");
    }
}

//! Lua の壁紙の画像（#36）。先頭のコメントの `-- @image 名前 パス` で指定する。
//!
//! 読み直すたびに 1 回だけデコードし、すべての出力で使う。Lua の壁紙は EGL surface より先に
//! 読み込むことがあるので、読み込むときはデコードだけを行い、テクスチャーにするのは最初に
//! 描くとき（コンテキストが current のとき）にする。テクスチャーにしたら、デコードした画素は捨てる。

use std::cell::RefCell;
use std::path::Path;

use crate::directive::{ImageSpec, Sampler};
use crate::image::Image;
use crate::source::Source;
use crate::texture::{self, Texture};

/// デコードする画像の縦横の上限。GPU の上限は、テクスチャーにするときに確かめる。
const MAX_DECODE_SIZE: u32 = 16384;

/// 1 つの壁紙の画像。`Script` の描画関数は、ここでの番号で画像を指す。
#[derive(Default)]
pub struct Images {
    entries: Vec<Entry>,
    /// テクスチャーにした結果。まだなら None。失敗したら、そのエラーを返し続ける。
    state: RefCell<Option<Result<Vec<Texture>, String>>>,
}

struct Entry {
    name: String,
    sampler: Sampler,
    width: u32,
    height: u32,
    /// エラーの前置き（`<壁紙のファイル>:<行>: <画像のパス>`）。
    origin: String,
    /// 色にアルファを掛けた画素。テクスチャーにしたら捨てる。
    pixels: RefCell<Option<Image>>,
}

impl Images {
    /// 指定された画像をデコードする。どれか 1 つでも読めなければエラーを返す。
    /// エラーは `<壁紙のファイル>:<行>: <画像のパス>: <内容>` の形。
    pub fn load(source: &Source, specs: &[ImageSpec], wallpaper: &Path) -> Result<Self, String> {
        for spec in specs {
            source
                .check_path(&spec.path)
                .map_err(|e| format!("{}:{}: {e}", wallpaper.display(), spec.line))?;
        }
        let decoded = crate::image::load_all(source, specs, wallpaper, MAX_DECODE_SIZE);
        let mut list = Vec::with_capacity(specs.len());
        for (spec, img) in specs.iter().zip(decoded) {
            let img = img.map_err(|e| format!("{}:{}: {e}", wallpaper.display(), spec.line))?;
            let origin = format!(
                "{}:{}: {}",
                wallpaper.display(),
                spec.line,
                spec.resolve(wallpaper).display()
            );
            list.push((spec.name.clone(), spec.sampler, img, origin));
        }
        Ok(Self::from_decoded(list))
    }

    fn from_decoded(list: Vec<(String, Sampler, Image, String)>) -> Self {
        let entries = list
            .into_iter()
            .map(|(name, sampler, mut img, origin)| {
                img.premultiply();
                Entry {
                    name,
                    sampler,
                    width: img.width,
                    height: img.height,
                    origin,
                    pixels: RefCell::new(Some(img)),
                }
            })
            .collect();
        Self {
            entries,
            state: RefCell::new(None),
        }
    }

    /// テスト用: デコード済みの画像から作る。
    #[cfg(test)]
    pub fn from_images(list: Vec<(&str, Sampler, Image)>) -> Self {
        Self::from_decoded(
            list.into_iter()
                .map(|(n, s, img)| (n.to_string(), s, img, format!("test: {n}")))
                .collect(),
        )
    }

    /// (名前, 幅, 高さ) を番号の順に返す。
    pub fn sizes(&self) -> impl Iterator<Item = (&str, u32, u32)> {
        self.entries
            .iter()
            .map(|e| (e.name.as_str(), e.width, e.height))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// まだなら、すべての画像をテクスチャーにする。コンテキストが current のときに呼ぶ。
    pub fn upload(&self, gl: &glow::Context) -> Result<(), String> {
        if let Some(state) = &*self.state.borrow() {
            return state.as_ref().map(|_| ()).map_err(Clone::clone);
        }
        let max = texture::max_size(gl);
        let mut textures = Vec::with_capacity(self.entries.len());
        let mut result = Ok(());
        for e in &self.entries {
            if e.width > max || e.height > max {
                result = Err(format!(
                    "{}: image is too large ({}x{}, max {max}x{max} on this GPU)",
                    e.origin, e.width, e.height
                ));
                break;
            }
            let pixels = e.pixels.borrow();
            let Some(img) = pixels.as_ref() else {
                result = Err(format!(
                    "{}: internal: image was already released",
                    e.origin
                ));
                break;
            };
            match texture::upload(gl, img, e.sampler) {
                Ok(t) => textures.push(t),
                Err(err) => {
                    result = Err(format!("{}: {err}", e.origin));
                    break;
                }
            }
        }
        // 成功しても失敗しても、この画素はもう使わない（失敗は読み直すまで返し続ける）。
        for e in &self.entries {
            e.pixels.borrow_mut().take();
        }
        let state = match result {
            Ok(()) => Ok(textures),
            Err(e) => {
                for t in textures {
                    texture::delete(gl, t);
                }
                Err(e)
            }
        };
        let r = state.as_ref().map(|_| ()).map_err(Clone::clone);
        *self.state.borrow_mut() = Some(state);
        r
    }

    /// i 番目の画像のテクスチャー。`upload` が成功した後に呼ぶ。
    pub fn texture(&self, i: usize) -> Option<glow::Texture> {
        match &*self.state.borrow() {
            Some(Ok(t)) => t.get(i).map(|t| t.tex),
            _ => None,
        }
    }

    pub fn delete(self, gl: &glow::Context) {
        if let Some(Ok(textures)) = self.state.into_inner() {
            for t in textures {
                texture::delete(gl, t);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directive::{Syntax, parse};

    #[test]
    fn load_reports_the_line_and_path_of_a_missing_image() {
        let src = "-- @image a images/plasma.jpg\n-- @image b images/missing.png\n";
        let specs = parse(src, Syntax::Lua).unwrap();
        let e = Images::load(&Source::disk(), &specs, Path::new("examples/x.lua"))
            .err()
            .unwrap();
        assert!(
            e.starts_with("examples/x.lua:2: examples/images/missing.png: "),
            "{e}"
        );
    }

    #[test]
    fn load_keeps_the_order_and_sizes() {
        let specs = parse("-- @image p images/plasma.jpg\n", Syntax::Lua).unwrap();
        let images = Images::load(&Source::disk(), &specs, Path::new("examples/x.lua")).unwrap();
        assert_eq!(images.sizes().collect::<Vec<_>>(), [("p", 512, 512)]);
    }

    #[test]
    fn load_reads_images_from_a_zip() {
        use crate::source::tests::Dir;
        use crate::zipfile::test_zip::{File, build};
        let jpg = std::fs::read("examples/images/plasma.jpg").unwrap();
        let src = "-- @image p images/plasma.jpg\n";
        let d = Dir::new("lua-zip");
        let zip = d.write(
            "w.zip",
            &build(&[
                File::new("w/main.lua", src.as_bytes()),
                File::new("w/images/plasma.jpg", &jpg).stored(),
            ]),
        );
        let source = Source::open(&zip).unwrap();
        assert!(!source.is_shader());
        let specs = parse(src, Syntax::Lua).unwrap();
        let images = Images::load(&source, &specs, source.entry()).unwrap();
        assert_eq!(images.sizes().collect::<Vec<_>>(), [("p", 512, 512)]);

        let specs = parse("\n-- @image p ~/plasma.jpg\n", Syntax::Lua).unwrap();
        let e = Images::load(&source, &specs, source.entry()).err().unwrap();
        assert!(
            e.ends_with("w.zip/w/main.lua:2: ~/plasma.jpg: absolute paths and ~/ cannot be used in a zip file"),
            "{e}"
        );
    }

    #[test]
    fn premultiply_scales_color_by_alpha() {
        let mut img = Image {
            width: 2,
            height: 1,
            rgba: vec![255, 128, 10, 128, 200, 200, 200, 0],
        };
        img.premultiply();
        assert_eq!(img.rgba, [128, 64, 5, 128, 0, 0, 0, 0]);
    }
}

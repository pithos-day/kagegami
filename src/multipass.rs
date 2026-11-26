//! シェーダーの壁紙のマルチパス（#57）: Image のファイルと、そこから指定された
//! バッファー（`@bufferA`〜`@bufferD`）・Common（`@common`）のファイルを読み、指定を確かめる。
//!
//! GPU には触れない。描くのは `shader.rs`、バッファーのテクスチャーは `buffers.rs`。

use std::path::{Path, PathBuf};

use crate::directive::{self, BUFFER_NAMES, BufferRef, GlslHeader, ImageSpec, NoiseRef};
use crate::source::Source;

/// 1 つのパスのファイル。
#[derive(Debug)]
pub struct PassFile {
    pub path: PathBuf,
    pub src: String,
    pub header: GlslHeader,
}

impl PassFile {
    /// チャンネルの番号ごとの、バッファーの指定。
    pub fn buffer_refs(&self) -> [Option<&BufferRef>; 4] {
        let mut refs = [None; 4];
        for r in &self.header.buffer_refs {
            refs[r.channel] = Some(r);
        }
        refs
    }

    pub fn images(&self) -> &[ImageSpec] {
        &self.header.images
    }

    pub fn noise_refs(&self) -> &[NoiseRef] {
        &self.header.noise_refs
    }

    /// チャンネルごとの、シェーダーで宣言する sampler の型。ノイズのチャンネルだけ 3D にする。
    pub fn channel_dims(&self) -> [crate::shader::Dim; 4] {
        let mut dims = [crate::shader::Dim::D2; 4];
        for r in &self.header.noise_refs {
            dims[r.channel] = crate::shader::Dim::D3;
        }
        dims
    }
}

/// シェーダーの壁紙を作るファイルのすべて。
#[derive(Debug)]
pub struct ShaderFiles {
    pub image: PassFile,
    /// 添字はバッファーの添字（A = 0〜D = 3）。
    pub buffers: [Option<PassFile>; 4],
    pub common: Option<(PathBuf, String)>,
}

/// `path` のファイルの中身 `src` の、先頭のコメントの指定を読む。指定のパスがこの壁紙で使えるかも
/// 確かめる（zip の中では `~/` と絶対パスを使えない）。
fn parse(source: &Source, path: &Path, src: &str) -> Result<GlslHeader, String> {
    let header = directive::parse_glsl(src).map_err(|e| format!("{}:{e}", path.display()))?;
    let decls = header.buffers.iter().flatten().chain(&header.common);
    let paths = decls
        .map(|d| (d.line, &d.path))
        .chain(header.images.iter().map(|s| (s.line, &s.path)));
    for (line, raw) in paths {
        source
            .check_path(raw)
            .map_err(|e| format!("{}:{line}: {e}", path.display()))?;
    }
    Ok(header)
}

impl ShaderFiles {
    /// Image のファイル `path` と、そこから指定されたファイルを読む。
    /// 見張るべきファイル（読めなかったものも含む）を、見つけた順に `watch` へ足す。
    pub fn read(source: &Source, path: &Path, watch: &mut Vec<PathBuf>) -> Result<Self, String> {
        let src = source.read_to_string(path)?;
        let header = parse(source, path, &src)?;
        let image = PassFile {
            path: path.to_path_buf(),
            src,
            header,
        };
        watch_images(&image, watch);

        let mut buffers: [Option<PassFile>; 4] = Default::default();
        for (i, decl) in image.header.buffers.iter().enumerate() {
            let Some(decl) = decl else { continue };
            let p = decl.resolve(path);
            watch.push(p.clone());
            let src = source.read_to_string(&p).map_err(|e| {
                format!(
                    "{}:{}: @{}: {e}",
                    path.display(),
                    decl.line,
                    BUFFER_NAMES[i]
                )
            })?;
            let header = parse(source, &p, &src)?;
            if let Some((line, name)) = header.first_pass_decl() {
                return Err(format!(
                    "{}:{line}: {name} can only be written in the Image file ({})",
                    p.display(),
                    path.display()
                ));
            }
            let pass = PassFile {
                path: p,
                src,
                header,
            };
            watch_images(&pass, watch);
            buffers[i] = Some(pass);
        }

        let common = match &image.header.common {
            Some(decl) => {
                let p = decl.resolve(path);
                watch.push(p.clone());
                let src = source
                    .read_to_string(&p)
                    .map_err(|e| format!("{}:{}: @common: {e}", path.display(), decl.line))?;
                let header = parse(source, &p, &src)?;
                if let Some((line, name)) = header.first_pass_decl() {
                    return Err(format!(
                        "{}:{line}: {name} cannot be written in the Common file",
                        p.display()
                    ));
                }
                if let Some((line, ch)) = header.first_channel() {
                    return Err(format!(
                        "{}:{line}: @iChannel{ch} cannot be written in the Common file",
                        p.display()
                    ));
                }
                Some((p, src))
            }
            None => None,
        };

        let files = Self {
            image,
            buffers,
            common,
        };
        for pass in files.passes() {
            for r in &pass.header.buffer_refs {
                if files.buffers[r.buffer].is_none() {
                    let name = BUFFER_NAMES[r.buffer];
                    return Err(format!(
                        "{}:{}: @iChannel{}: {name} is not declared (write // @{name} <file> in {})",
                        pass.path.display(),
                        r.line,
                        r.channel,
                        files.image.path.display()
                    ));
                }
            }
        }
        Ok(files)
    }

    /// Image と、宣言されたバッファーのパス。
    pub fn passes(&self) -> impl Iterator<Item = &PassFile> {
        std::iter::once(&self.image).chain(self.buffers.iter().flatten())
    }

    pub fn has_buffers(&self) -> bool {
        self.buffers.iter().any(Option::is_some)
    }

    /// どれかのパスが mipmap で読むバッファー。
    pub fn mipmapped_buffers(&self) -> [bool; 4] {
        let mut m = [false; 4];
        for pass in self.passes() {
            for r in &pass.header.buffer_refs {
                if r.sampler.filter == directive::Filter::Mipmap {
                    m[r.buffer] = true;
                }
            }
        }
        m
    }

    /// どれかのパスがバッファーを linear か mipmap で読むか。
    pub fn filters_buffers(&self) -> bool {
        self.passes().any(|p| {
            p.header
                .buffer_refs
                .iter()
                .any(|r| r.sampler.filter != directive::Filter::Nearest)
        })
    }

    /// 画像の読み直しに使う鍵。どれかのファイルの中身か、ファイルの場所が変われば変わる。
    pub fn cache_key(&self) -> String {
        let mut key = String::new();
        for pass in self.passes() {
            key.push_str(&pass.path.display().to_string());
            key.push('\0');
            key.push_str(&pass.src);
            key.push('\0');
        }
        key
    }
}

fn watch_images(pass: &PassFile, watch: &mut Vec<PathBuf>) {
    watch.extend(pass.images().iter().map(|s| s.resolve(&pass.path)));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストごとに別の一時ディレクトリにファイルを書く。
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let d = std::env::temp_dir()
                .join(format!("kagegami-multipass-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }

        fn write(&self, name: &str, src: &str) -> PathBuf {
            self.write_bytes(name, src.as_bytes())
        }

        fn write_bytes(&self, name: &str, data: &[u8]) -> PathBuf {
            let p = self.0.join(name);
            std::fs::write(&p, data).unwrap();
            p
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn read(image: &Path, watch: &mut Vec<PathBuf>) -> Result<ShaderFiles, String> {
        ShaderFiles::read(&Source::open(image).unwrap(), image, watch)
    }

    const MAIN: &str = "void mainImage(out vec4 c, in vec2 p) { c = vec4(0.0); }\n";

    #[test]
    fn reads_buffers_and_common_and_watches_every_file() {
        let d = Dir::new("ok");
        d.write(
            "a.glsl",
            &format!("// @iChannel0 bufferA\n// @iChannel1 noise.png\n{MAIN}"),
        );
        d.write(
            "c.glsl",
            &format!("// @iChannel0 bufferA filter=mipmap\n{MAIN}"),
        );
        d.write("common.glsl", "float k() { return 1.0; }\n");
        let image = d.write(
            "main.glsl",
            &format!(
                "// @bufferA a.glsl\n// @bufferC c.glsl\n// @common common.glsl\n\
                 // @iChannel0 bufferC filter=nearest wrap=repeat\n{MAIN}"
            ),
        );
        let mut watch = vec![];
        let f = read(&image, &mut watch).unwrap();
        assert!(f.buffers[0].is_some() && f.buffers[1].is_none() && f.buffers[2].is_some());
        assert_eq!(f.common.as_ref().unwrap().1, "float k() { return 1.0; }\n");
        assert_eq!(f.mipmapped_buffers(), [true, false, false, false]);
        assert!(f.filters_buffers());
        let refs = f.image.buffer_refs();
        assert_eq!(refs[0].unwrap().buffer, 2);
        assert!(refs[1].is_none());
        // 画像は、その指定を書いたファイルのディレクトリから探す。
        assert_eq!(
            watch,
            vec![
                d.0.join("a.glsl"),
                d.0.join("noise.png"),
                d.0.join("c.glsl"),
                d.0.join("common.glsl"),
            ]
        );
    }

    #[test]
    fn single_pass_shader_has_no_buffers() {
        let d = Dir::new("single");
        let image = d.write("main.glsl", MAIN);
        let f = read(&image, &mut vec![]).unwrap();
        assert!(!f.has_buffers());
        assert!(!f.filters_buffers());
        assert!(f.common.is_none());
    }

    #[test]
    fn cache_key_changes_when_any_pass_changes() {
        let d = Dir::new("key");
        d.write("a.glsl", MAIN);
        let image = d.write("main.glsl", &format!("// @bufferA a.glsl\n{MAIN}"));
        let k1 = read(&image, &mut vec![]).unwrap().cache_key();
        d.write("a.glsl", &format!("// @iChannel0 x.png\n{MAIN}"));
        let k2 = read(&image, &mut vec![]).unwrap().cache_key();
        assert_ne!(k1, k2);
    }

    #[test]
    fn errors_name_the_file_and_line() {
        let d = Dir::new("err");
        // (Image のファイルの先頭, ほかのファイルの (名前, 先頭), 期待するエラー)
        type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);
        let cases: &[Case] = &[
            (
                "// @iChannel0 bufferB\n",
                &[],
                "main.glsl:1: @iChannel0: bufferB is not declared (write // @bufferB <file> in",
            ),
            (
                "// @bufferA a.glsl\n",
                &[("a.glsl", "// @iChannel2 bufferD\n")],
                "a.glsl:1: @iChannel2: bufferD is not declared",
            ),
            (
                "// @bufferA a.glsl\n",
                &[("a.glsl", "// x\n// @bufferB b.glsl\n")],
                "a.glsl:2: @bufferB can only be written in the Image file",
            ),
            (
                "// @bufferA a.glsl\n",
                &[("a.glsl", "// @common c.glsl\n")],
                "a.glsl:1: @common can only be written in the Image file",
            ),
            (
                "// @common c.glsl\n",
                &[("c.glsl", "// @iChannel1 x.png\n")],
                "c.glsl:1: @iChannel1 cannot be written in the Common file",
            ),
            (
                "// @common c.glsl\n",
                &[("c.glsl", "// @bufferA a.glsl\n")],
                "c.glsl:1: @bufferA cannot be written in the Common file",
            ),
            (
                "\n\n// @bufferD missing.glsl\n",
                &[],
                "main.glsl:3: @bufferD: ",
            ),
            ("// @common missing.glsl\n", &[], "main.glsl:1: @common: "),
        ];
        for (main, others, want) in cases {
            for (name, src) in *others {
                d.write(name, &format!("{src}{MAIN}"));
            }
            let image = d.write("main.glsl", &format!("{main}{MAIN}"));
            let e = read(&image, &mut vec![]).unwrap_err();
            assert!(e.contains(want), "{main:?}: {e}");
            for (name, _) in *others {
                std::fs::remove_file(d.0.join(name)).unwrap();
            }
        }
    }

    #[test]
    fn unreadable_buffer_file_is_still_watched() {
        let d = Dir::new("watch");
        let image = d.write("main.glsl", &format!("// @bufferA a.glsl\n{MAIN}"));
        let mut watch = vec![];
        assert!(read(&image, &mut watch).is_err());
        assert_eq!(watch, vec![d.0.join("a.glsl")]);
    }

    #[test]
    fn reads_buffers_from_a_zip_and_rejects_paths_outside() {
        use crate::zipfile::test_zip::{File, build};
        let d = Dir::new("zip");
        let main = format!(
            "// @bufferA lib/a.glsl\n// @common common.glsl\n// @iChannel0 bufferA\n{MAIN}"
        );
        let buf = format!("// @iChannel1 ../img/n.png\n{MAIN}");
        let zip = d.write_bytes(
            "w.zip",
            &build(&[
                File::new("main.glsl", main.as_bytes()),
                File::new("lib/a.glsl", buf.as_bytes()),
                File::new("common.glsl", b"float k;\n"),
            ]),
        );
        let source = Source::open(&zip).unwrap();
        let mut watch = vec![];
        let f = ShaderFiles::read(&source, source.entry(), &mut watch).unwrap();
        assert_eq!(f.buffers[0].as_ref().unwrap().path, zip.join("lib/a.glsl"));
        assert_eq!(f.common.as_ref().unwrap().1, "float k;\n");
        // 画像は zip の中のパスで表す（見張るのは zip ファイルだけ。`Source::watch_paths` を参照）。
        assert!(watch.contains(&zip.join("lib/../img/n.png")));

        for (header, want) in [
            (
                "// @bufferA ~/a.glsl\n",
                "main.glsl:1: ~/a.glsl: absolute paths and ~/ cannot be used in a zip file",
            ),
            (
                "// @iChannel0 /etc/x.png\n",
                "main.glsl:1: /etc/x.png: absolute paths",
            ),
            ("// @common ../../c.glsl\n", "main.glsl:1: @common: "),
        ] {
            let zip = d.write_bytes(
                "bad.zip",
                &build(&[File::new(
                    "w/main.glsl",
                    format!("{header}{MAIN}").as_bytes(),
                )]),
            );
            let source = Source::open(&zip).unwrap();
            let e = ShaderFiles::read(&source, source.entry(), &mut vec![]).unwrap_err();
            assert!(e.contains(want), "{e}");
        }
    }
}

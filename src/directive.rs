//! 壁紙のファイルの先頭のコメントに書く、画像（#36）とバッファー（#57）の指定。
//!
//! ```glsl
//! // @iChannel0 photo.jpg
//! // @iChannel1 "my noise.png" filter=nearest wrap=clamp vflip=false
//! // @bufferA a.glsl
//! // @common common.glsl
//! // @iChannel2 bufferA filter=nearest
//! ```
//!
//! ```lua
//! -- @image bg photo.jpg filter=linear
//! ```
//!
//! - ファイルの先頭から、コメント（行コメント・ブロックコメント）と空行が続く間に書く。
//!   それより後ろの行コメントに書いた指定はエラーにする
//! - 大文字・小文字を問わず `@ichannel` か `@image` で始まる名前（シェーダーでは `@buffer`・`@common` も）
//!   だけを指定として確かめる。それ以外の `@author` などは、ただのコメントとして無視する

use std::path::{Path, PathBuf};

/// どちらの壁紙のファイルか。コメントの書き方と、使える指定が違う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Syntax {
    Glsl,
    Lua,
}

impl Syntax {
    fn line_comment(self) -> &'static str {
        match self {
            Syntax::Glsl => "//",
            Syntax::Lua => "--",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Nearest,
    Linear,
    Mipmap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    Clamp,
    Repeat,
}

/// テクスチャーの設定。シェーダーの既定は filter=mipmap、wrap=repeat、vflip=true。
/// Lua の既定は filter=mipmap、wrap=clamp で、vflip は指定できない（常に false）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sampler {
    pub filter: Filter,
    pub wrap: Wrap,
    /// 画像の上下を反転してテクスチャーにする。シェーダーの座標は左下が原点なので、
    /// true なら画像の上が上に来る。Lua の壁紙では指定できない（常に false）。
    pub vflip: bool,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            filter: Filter::Mipmap,
            wrap: Wrap::Repeat,
            vflip: true,
        }
    }
}

/// 1 つの画像の指定。
#[derive(Clone, Debug, PartialEq)]
pub struct ImageSpec {
    /// シェーダーはチャンネルの番号（"0"〜"3"）、Lua は画像の名前。
    pub name: String,
    /// 書かれたままのパス。
    pub path: String,
    pub sampler: Sampler,
    /// 指定を書いた行（1 始まり）。
    pub line: usize,
}

impl ImageSpec {
    /// 壁紙のファイルからの相対パスや `~/` を解決したパス。
    pub fn resolve(&self, wallpaper: &Path) -> PathBuf {
        resolve_path(&self.path, wallpaper, std::env::var_os("HOME").as_deref())
    }
}

/// バッファーの名前（`bufferA`〜`bufferD`）。添字は 0〜3。
pub const BUFFER_NAMES: [&str; 4] = ["bufferA", "bufferB", "bufferC", "bufferD"];

/// `@iChannelN bufferA` のように、チャンネルにバッファーを渡す指定。
#[derive(Clone, Debug, PartialEq)]
pub struct BufferRef {
    /// チャンネルの番号（0〜3）。
    pub channel: usize,
    /// バッファーの添字（A = 0〜D = 3）。
    pub buffer: usize,
    /// filter と wrap だけを使う。既定は linear・clamp（Shadertoy でバッファーを選んだときの既定）。
    pub sampler: Sampler,
    pub line: usize,
}

/// `@bufferA a.glsl`・`@common common.glsl` のように、別のファイルを指す指定。
#[derive(Clone, Debug, PartialEq)]
pub struct FileDecl {
    /// 書かれたままのパス。
    pub path: String,
    pub line: usize,
}

impl FileDecl {
    pub fn resolve(&self, wallpaper: &Path) -> PathBuf {
        resolve_path(&self.path, wallpaper, std::env::var_os("HOME").as_deref())
    }
}

/// シェーダーの壁紙のファイルの指定。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlslHeader {
    pub images: Vec<ImageSpec>,
    pub buffer_refs: Vec<BufferRef>,
    /// `@bufferA`〜`@bufferD`。添字はバッファーの添字。
    pub buffers: [Option<FileDecl>; 4],
    pub common: Option<FileDecl>,
}

impl GlslHeader {
    /// `@bufferA`〜`D`・`@common` の、最初に書かれた行。どれもなければ None。
    pub fn first_pass_decl(&self) -> Option<(usize, String)> {
        let buffers = self.buffers.iter().enumerate().filter_map(|(i, d)| {
            d.as_ref()
                .map(|d| (d.line, format!("@{}", BUFFER_NAMES[i])))
        });
        let common = self.common.iter().map(|d| (d.line, "@common".to_string()));
        buffers.chain(common).min_by_key(|(line, _)| *line)
    }

    /// 画像とバッファーの指定のうち、最初に書かれた行。どれもなければ None。
    pub fn first_channel(&self) -> Option<(usize, usize)> {
        let images = self
            .images
            .iter()
            .map(|s| (s.line, s.name.parse().unwrap_or(0)));
        let refs = self.buffer_refs.iter().map(|r| (r.line, r.channel));
        images.chain(refs).min_by_key(|(line, _)| *line)
    }
}

/// 1 行の指定。
enum Item {
    Image(ImageSpec),
    BufferRef(BufferRef),
    Buffer(usize, FileDecl),
    Common(FileDecl),
}

/// 絶対パスはそのまま、先頭の `~/` は `$HOME` に置き換え、それ以外は壁紙のファイルのディレクトリから。
fn resolve_path(path: &str, wallpaper: &Path, home: Option<&std::ffi::OsStr>) -> PathBuf {
    if let (Some(rest), Some(home)) = (path.strip_prefix("~/"), home) {
        return Path::new(home).join(rest);
    }
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    wallpaper.parent().unwrap_or(Path::new("")).join(p)
}

/// Lua の壁紙のファイルから画像の指定を読む。エラーは `行番号: 内容` の形で返す。
/// シェーダーの壁紙は `parse_glsl` を使う（ここではバッファーの指定を受け付けない）。
pub fn parse(src: &str, syntax: Syntax) -> Result<Vec<ImageSpec>, String> {
    let mut specs = Vec::new();
    for item in items(src, syntax)? {
        match item {
            Item::Image(spec) => specs.push(spec),
            Item::BufferRef(r) => {
                return Err(format!("{}: buffers can only be used in shaders", r.line));
            }
            Item::Buffer(_, d) | Item::Common(d) => {
                return Err(format!("{}: buffers can only be used in shaders", d.line));
            }
        }
    }
    Ok(specs)
}

/// シェーダーの壁紙のファイルから、画像とバッファーの指定を読む。エラーは `行番号: 内容` の形で返す。
pub fn parse_glsl(src: &str) -> Result<GlslHeader, String> {
    let mut h = GlslHeader::default();
    for item in items(src, Syntax::Glsl)? {
        match item {
            Item::Image(spec) => h.images.push(spec),
            Item::BufferRef(r) => h.buffer_refs.push(r),
            Item::Buffer(i, d) => {
                if h.buffers[i].is_some() {
                    return Err(format!(
                        "{}: @{} is specified twice",
                        d.line, BUFFER_NAMES[i]
                    ));
                }
                h.buffers[i] = Some(d);
            }
            Item::Common(d) => {
                if h.common.is_some() {
                    return Err(format!("{}: @common is specified twice", d.line));
                }
                h.common = Some(d);
            }
        }
    }
    Ok(h)
}

/// 先頭のコメントの指定を、書かれた順に読む。
fn items(src: &str, syntax: Syntax) -> Result<Vec<Item>, String> {
    let mut specs: Vec<Item> = Vec::new();
    // 指定済みのチャンネル（シェーダー）・画像の名前（Lua）。
    let mut names: Vec<String> = Vec::new();
    let mut header = true;
    // ブロックコメントの中なら、その終わりの印。
    let mut in_block: Option<String> = None;
    for (i, raw) in src.lines().enumerate() {
        let line = i + 1;
        let t = raw.trim();
        if let Some(end) = &in_block {
            if let Some(pos) = t.find(end.as_str()) {
                let rest = t[pos + end.len()..].trim();
                in_block = None;
                if !rest.is_empty() {
                    header = false;
                }
            }
            continue;
        }
        let comment = t.strip_prefix(syntax.line_comment());
        if header {
            if t.is_empty() {
                continue;
            }
            if let Some(end) = block_comment_start(t, syntax) {
                // 同じ行で閉じていれば、その後ろを確かめる。
                let body = &t[block_open_len(t, syntax)..];
                match body.find(end.as_str()) {
                    Some(pos) => {
                        if !body[pos + end.len()..].trim().is_empty() {
                            header = false;
                        }
                    }
                    None => in_block = Some(end),
                }
                continue;
            }
            if let Some(c) = comment {
                if let Some(item) = directive(c, syntax, line)? {
                    let name = match &item {
                        Item::Image(spec) => Some(spec.name.clone()),
                        Item::BufferRef(r) => Some(r.channel.to_string()),
                        _ => None,
                    };
                    if let Some(name) = name {
                        if names.contains(&name) {
                            return Err(format!(
                                "{line}: {} is specified twice",
                                spec_label(&name, syntax)
                            ));
                        }
                        names.push(name);
                    }
                    specs.push(item);
                }
                continue;
            }
            header = false;
        }
        // 先頭のコメントより後ろの指定は、書いた場所の間違いとしてエラーにする。
        if let Some(c) = comment
            && let Some(name) = directive_name(c, syntax)
        {
            return Err(format!(
                "{line}: @{name} must be in the comments at the top of the file"
            ));
        }
    }
    Ok(specs)
}

fn spec_label(name: &str, syntax: Syntax) -> String {
    match syntax {
        Syntax::Glsl => format!("@iChannel{name}"),
        Syntax::Lua => format!("@image {name}"),
    }
}

/// ブロックコメントの始まりなら、その終わりの印を返す。
fn block_comment_start(t: &str, syntax: Syntax) -> Option<String> {
    match syntax {
        Syntax::Glsl => t.starts_with("/*").then(|| "*/".to_string()),
        Syntax::Lua => {
            // --[[ ... ]] と --[==[ ... ]==]
            let rest = t.strip_prefix("--[")?;
            let eq = rest.chars().take_while(|&c| c == '=').count();
            rest[eq..]
                .starts_with('[')
                .then(|| format!("]{}]", "=".repeat(eq)))
        }
    }
}

fn block_open_len(t: &str, syntax: Syntax) -> usize {
    match syntax {
        Syntax::Glsl => 2,
        Syntax::Lua => 4 + t[3..].chars().take_while(|&c| c == '=').count(),
    }
}

/// 行コメントの中身が `@ichannel…` か `@image…`（シェーダーでは `@buffer…`・`@common…` も）で
/// 始まっていれば、その名前。
fn directive_name(comment: &str, syntax: Syntax) -> Option<&str> {
    let rest = comment.trim_start().strip_prefix('@')?;
    let name = rest.split_whitespace().next().unwrap_or("");
    let lower = name.to_ascii_lowercase();
    let known = lower.starts_with("ichannel")
        || lower.starts_with("image")
        || (syntax == Syntax::Glsl && (lower.starts_with("buffer") || lower.starts_with("common")));
    known.then_some(name)
}

/// `bufferA`〜`bufferD` ならバッファーの添字。`BufferA` のような書き間違いはエラーにする
/// （画像のファイルの名前として探して、分かりにくいエラーになるのを避ける）。
fn buffer_index(token: &str) -> Result<Option<usize>, String> {
    if let Some(i) = BUFFER_NAMES.iter().position(|n| *n == token) {
        return Ok(Some(i));
    }
    let lower = token.to_ascii_lowercase();
    if let Some(i) = BUFFER_NAMES
        .iter()
        .position(|n| n.to_ascii_lowercase() == lower)
    {
        return Err(format!(
            "unknown buffer {token:?} (write {}; use ./{token} for a file)",
            BUFFER_NAMES[i]
        ));
    }
    Ok(None)
}

/// `@bufferA a.glsl`・`@common common.glsl` を読む。
fn file_decl(name: &str, args: Vec<String>, line: usize) -> Result<Item, String> {
    let err = |msg: String| format!("{line}: {msg}");
    let target = if name == "common" {
        None
    } else {
        match name.strip_prefix("buffer") {
            Some(n @ ("A" | "B" | "C" | "D")) => Some((n.as_bytes()[0] - b'A') as usize),
            _ if name.to_ascii_lowercase().starts_with("buffer") => {
                return Err(err(format!(
                    "unknown directive @{name} (use @bufferA to @bufferD)"
                )));
            }
            _ => return Err(err(format!("unknown directive @{name} (use @common)"))),
        }
    };
    let mut args = args.into_iter();
    let path = args
        .next()
        .ok_or_else(|| err(format!("@{name}: expected a path")))?;
    if let Some(extra) = args.next() {
        return Err(err(format!("@{name}: unexpected argument {extra:?}")));
    }
    let decl = FileDecl { path, line };
    Ok(match target {
        Some(i) => Item::Buffer(i, decl),
        None => Item::Common(decl),
    })
}

/// 行コメント 1 行を読む。指定でなければ None。
fn directive(comment: &str, syntax: Syntax, line: usize) -> Result<Option<Item>, String> {
    let Some(name) = directive_name(comment, syntax) else {
        return Ok(None);
    };
    let err = |msg: String| format!("{line}: {msg}");
    let args = tokenize(comment.trim_start()[1 + name.len()..].trim()).map_err(err)?;
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("buffer") || lower.starts_with("common") {
        return file_decl(name, args, line).map(Some);
    }
    let mut args = args.into_iter();
    let spec_name = match syntax {
        Syntax::Glsl => match name.strip_prefix("iChannel") {
            Some(n @ ("0" | "1" | "2" | "3")) => n.to_string(),
            _ => {
                return Err(err(format!(
                    "unknown directive @{name} (use @iChannel0 to @iChannel3)"
                )));
            }
        },
        Syntax::Lua => {
            if name != "image" {
                return Err(err(format!("unknown directive @{name} (use @image)")));
            }
            args.next()
                .ok_or_else(|| err("@image: expected a name and a path".into()))?
        }
    };
    let label = spec_label(&spec_name, syntax);
    let path = args
        .next()
        .ok_or_else(|| err(format!("{label}: expected a path")))?;
    let buffer = match syntax {
        Syntax::Glsl => buffer_index(&path).map_err(|e| err(format!("{label}: {e}")))?,
        Syntax::Lua => None,
    };
    // Lua の画像は 1 枚の絵として置くことが多いので、拡大したときに反対側の端が縁に混ざらないよう
    // clamp を既定にする（#36）。
    let mut sampler = match buffer {
        // Shadertoy でバッファーを選んだときの既定（#57）。
        Some(_) => Sampler {
            filter: Filter::Linear,
            wrap: Wrap::Clamp,
            vflip: false,
        },
        None => Sampler {
            wrap: match syntax {
                Syntax::Glsl => Wrap::Repeat,
                Syntax::Lua => Wrap::Clamp,
            },
            vflip: syntax == Syntax::Glsl,
            ..Sampler::default()
        },
    };
    for opt in args {
        let (key, value) = opt
            .split_once('=')
            .ok_or_else(|| err(format!("{label}: expected key=value, got {opt:?}")))?;
        let bad = || err(format!("{label}: unknown value {value:?} for {key}"));
        match key {
            "filter" => {
                sampler.filter = match value {
                    "nearest" => Filter::Nearest,
                    "linear" => Filter::Linear,
                    "mipmap" => Filter::Mipmap,
                    _ => return Err(bad()),
                }
            }
            "wrap" => {
                sampler.wrap = match value {
                    "clamp" => Wrap::Clamp,
                    "repeat" => Wrap::Repeat,
                    _ => return Err(bad()),
                }
            }
            "vflip" if buffer.is_some() => {
                return Err(err(format!("{label}: vflip cannot be used with a buffer")));
            }
            "vflip" if syntax == Syntax::Glsl => {
                sampler.vflip = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err(bad()),
                }
            }
            _ => return Err(err(format!("{label}: unknown option {key:?}"))),
        }
    }
    if let Some(buffer) = buffer {
        return Ok(Some(Item::BufferRef(BufferRef {
            channel: spec_name.parse().unwrap_or(0),
            buffer,
            sampler,
            line,
        })));
    }
    Ok(Some(Item::Image(ImageSpec {
        name: spec_name,
        path,
        sampler,
        line,
    })))
}

/// 空白で区切る。`"` で囲んだところは空白を含めて 1 つにする。
fn tokenize(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut tok = String::new();
        if c == '"' {
            chars.next();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some(c) => tok.push(c),
                    None => return Err("unterminated \"".into()),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                tok.push(c);
                chars.next();
            }
        }
        out.push(tok);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glsl(src: &str) -> Result<Vec<ImageSpec>, String> {
        parse_glsl(src).map(|h| h.images)
    }

    fn lua(src: &str) -> Result<Vec<ImageSpec>, String> {
        parse(src, Syntax::Lua)
    }

    #[test]
    fn reads_channels_with_defaults_and_options() {
        let s = glsl(
            "// @iChannel0 photo.jpg\n\
             // @iChannel2 \"my noise.png\" filter=nearest wrap=clamp vflip=false\n\
             void mainImage(out vec4 c, in vec2 p) {}\n",
        )
        .unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "0");
        assert_eq!(s[0].path, "photo.jpg");
        assert_eq!(s[0].sampler, Sampler::default());
        assert_eq!(s[0].line, 1);
        assert_eq!(s[1].name, "2");
        assert_eq!(s[1].path, "my noise.png");
        assert_eq!(
            s[1].sampler,
            Sampler {
                filter: Filter::Nearest,
                wrap: Wrap::Clamp,
                vflip: false
            }
        );
    }

    #[test]
    fn header_may_contain_block_comments_and_blank_lines() {
        let s = glsl(
            "/*\n * MIT License\n */\n\n// The MIT License\n/* one line */\n// @iChannel1 a.png\nvoid mainImage(out vec4 c, in vec2 p) {}\n",
        )
        .unwrap();
        assert_eq!(s[0].name, "1");
        let s = lua("--[[\n license\n]]\n--[==[ x ]==]\n-- @image bg a.png\nfunction draw() end\n")
            .unwrap();
        assert_eq!(s[0].name, "bg");
    }

    #[test]
    fn directive_after_the_header_is_an_error() {
        let e = glsl("void f() {}\n// @iChannel0 a.png\n").unwrap_err();
        assert!(
            e.starts_with("2: @iChannel0 must be in the comments at the top"),
            "{e}"
        );
        // 同じ行でブロックコメントの後ろにコードがあれば、そこで先頭のコメントは終わる。
        let e = glsl("/* x */ float a;\n// @iChannel0 a.png\n").unwrap_err();
        assert!(e.starts_with("2:"), "{e}");
        let e = lua("local x = 1\n-- @image bg a.png\n").unwrap_err();
        assert!(e.starts_with("2: @image must be"), "{e}");
        // 普通のコメントは、後ろにあってもよい。
        glsl("void f() {}\n// @author someone\n// see iChannel0\n").unwrap();
    }

    #[test]
    fn other_at_names_are_plain_comments() {
        assert!(glsl("// @author me\n// @license MIT\n").unwrap().is_empty());
        assert!(lua("-- @author me\n").unwrap().is_empty());
    }

    #[test]
    fn misspelled_directives_are_errors() {
        for (src, want) in [
            ("// @iChannel4 a.png", "unknown directive @iChannel4"),
            ("// @ichannel0 a.png", "unknown directive @ichannel0"),
            ("// @iChannel a.png", "unknown directive @iChannel"),
            ("// @image bg a.png", "unknown directive @image"),
        ] {
            let e = glsl(src).unwrap_err();
            assert!(e.contains(want), "{src}: {e}");
        }
        for (src, want) in [
            ("-- @images bg a.png", "unknown directive @images"),
            ("-- @Image bg a.png", "unknown directive @Image"),
            ("-- @iChannel0 a.png", "unknown directive @iChannel0"),
        ] {
            let e = lua(src).unwrap_err();
            assert!(e.contains(want), "{src}: {e}");
        }
    }

    #[test]
    fn bad_arguments_are_errors() {
        for (src, want) in [
            ("// @iChannel0", "@iChannel0: expected a path"),
            (
                "// @iChannel0 a.png filter=cubic",
                "unknown value \"cubic\" for filter",
            ),
            (
                "// @iChannel0 a.png wrap=mirror",
                "unknown value \"mirror\" for wrap",
            ),
            (
                "// @iChannel0 a.png vflip=yes",
                "unknown value \"yes\" for vflip",
            ),
            ("// @iChannel0 a.png size=2", "unknown option \"size\""),
            ("// @iChannel0 a.png nearest", "expected key=value"),
            ("// @iChannel0 \"a.png", "unterminated"),
            (
                "// @iChannel0 a.png\n// @iChannel0 b.png",
                "2: @iChannel0 is specified twice",
            ),
        ] {
            let e = glsl(src).unwrap_err();
            assert!(e.contains(want), "{src}: {e}");
        }
        for (src, want) in [
            ("-- @image", "expected a name and a path"),
            ("-- @image bg", "@image bg: expected a path"),
            // Lua の座標は左上が原点なので、vflip は受け付けない。
            ("-- @image bg a.png vflip=false", "unknown option \"vflip\""),
            (
                "-- @image bg a.png\n-- @image bg b.png",
                "@image bg is specified twice",
            ),
        ] {
            let e = lua(src).unwrap_err();
            assert!(e.contains(want), "{src}: {e}");
        }
    }

    #[test]
    fn reads_buffers_common_and_buffer_channels() {
        let h = parse_glsl(
            "// @bufferA a.glsl\n\
             // @bufferC \"my c.glsl\"\n\
             // @common common.glsl\n\
             // @iChannel0 bufferA\n\
             // @iChannel1 bufferC filter=mipmap wrap=repeat\n\
             // @iChannel2 photo.jpg\n\
             // @iChannel3 ./bufferB\n",
        )
        .unwrap();
        assert_eq!(h.buffers[0].as_ref().unwrap().path, "a.glsl");
        assert!(h.buffers[1].is_none());
        assert_eq!(h.buffers[2].as_ref().unwrap().path, "my c.glsl");
        assert_eq!(h.buffers[2].as_ref().unwrap().line, 2);
        assert_eq!(h.common.as_ref().unwrap().path, "common.glsl");
        assert_eq!(h.first_pass_decl(), Some((1, "@bufferA".to_string())));
        assert_eq!(h.first_channel(), Some((4, 0)));
        assert_eq!(h.buffer_refs.len(), 2);
        // バッファーの既定は Shadertoy と同じ linear・clamp。
        assert_eq!(
            h.buffer_refs[0],
            BufferRef {
                channel: 0,
                buffer: 0,
                sampler: Sampler {
                    filter: Filter::Linear,
                    wrap: Wrap::Clamp,
                    vflip: false
                },
                line: 4
            }
        );
        assert_eq!(h.buffer_refs[1].buffer, 2);
        assert_eq!(h.buffer_refs[1].sampler.filter, Filter::Mipmap);
        assert_eq!(h.buffer_refs[1].sampler.wrap, Wrap::Repeat);
        // 画像は今までどおり。./bufferB はファイルの名前。
        assert_eq!(h.images.len(), 2);
        assert_eq!(h.images[0].name, "2");
        assert_eq!(h.images[1].path, "./bufferB");
    }

    #[test]
    fn bad_buffer_directives_are_errors() {
        for (src, want) in [
            (
                "// @bufferE a.glsl",
                "unknown directive @bufferE (use @bufferA to @bufferD)",
            ),
            ("// @buffera a.glsl", "unknown directive @buffera"),
            ("// @BufferA a.glsl", "unknown directive @BufferA"),
            (
                "// @Common c.glsl",
                "unknown directive @Common (use @common)",
            ),
            ("// @bufferA", "@bufferA: expected a path"),
            ("// @common", "@common: expected a path"),
            (
                "// @bufferA a.glsl filter=linear",
                "@bufferA: unexpected argument",
            ),
            (
                "// @bufferA a.glsl\n// @bufferA b.glsl",
                "2: @bufferA is specified twice",
            ),
            (
                "// @common a.glsl\n// @common b.glsl",
                "2: @common is specified twice",
            ),
            (
                "// @iChannel0 bufferA vflip=false",
                "@iChannel0: vflip cannot be used with a buffer",
            ),
            (
                "// @iChannel0 BufferA",
                "@iChannel0: unknown buffer \"BufferA\" (write bufferA; use ./BufferA for a file)",
            ),
            (
                "// @iChannel1 bufferA\n// @iChannel1 a.png",
                "2: @iChannel1 is specified twice",
            ),
            (
                "// @iChannel0 bufferA filter=cubic",
                "unknown value \"cubic\" for filter",
            ),
            (
                "void f() {}\n// @bufferA a.glsl",
                "2: @bufferA must be in the comments at the top",
            ),
        ] {
            let e = parse_glsl(src).unwrap_err();
            assert!(e.contains(want), "{src}: {e}");
        }
        // Lua の壁紙では @buffer・@common はただのコメント。
        assert!(
            lua("-- @bufferA a.glsl\n-- @common c.glsl\n")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn lua_images_default_to_mipmap_clamp_without_flip() {
        let s =
            lua("-- @image bg a.png\n-- @image dots b.png filter=nearest wrap=repeat\n").unwrap();
        assert_eq!(
            s[0].sampler,
            Sampler {
                filter: Filter::Mipmap,
                wrap: Wrap::Clamp,
                vflip: false
            }
        );
        assert_eq!(s[1].name, "dots");
        assert_eq!(s[1].sampler.filter, Filter::Nearest);
        assert_eq!(s[1].sampler.wrap, Wrap::Repeat);
    }

    #[test]
    fn paths_resolve_from_the_wallpaper_directory() {
        let wp = Path::new("/w/walls/a.glsl");
        let home = Some(std::ffi::OsStr::new("/home/u"));
        assert_eq!(
            resolve_path("img/p.png", wp, home),
            Path::new("/w/walls/img/p.png")
        );
        assert_eq!(
            resolve_path("/abs/p.png", wp, home),
            Path::new("/abs/p.png")
        );
        assert_eq!(
            resolve_path("~/Pictures/p.png", wp, home),
            Path::new("/home/u/Pictures/p.png")
        );
        // ~ の後ろが / でなければ、ただの名前として扱う。
        assert_eq!(
            resolve_path("~x.png", wp, home),
            Path::new("/w/walls/~x.png")
        );
        // 壁紙がカレントディレクトリにあるとき。
        assert_eq!(
            resolve_path("p.png", Path::new("a.glsl"), home),
            Path::new("p.png")
        );
    }
}

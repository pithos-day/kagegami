//! Shadertoy 形式のシェーダーの壁紙（#11）。
//!
//! `mainImage(out vec4, in vec2)` を書いたファイルを GLSL ES 3.00 で包み、全画面に描く。
//! 1 フレームの GPU 時間を `EXT_disjoint_timer_query` で測り、長すぎたらエラーとして止める。
//! バッファー（Buffer A〜D）と Common を使う壁紙（#57）は、パスごとに 1 つのプログラムにする。
//! 組み込みの 3D ノイズ（#70）を渡すチャンネルは `sampler3D`、キューブマップ（#68）を渡すチャンネルは
//! `samplerCube` で宣言する。Cubemap A（#72）のパスは、`mainCubemap` を 6 面それぞれに描く。

use std::path::Path;
use std::time::Duration;

use glow::HasContext;

use crate::cubemap_pass::CubemapPass;
use crate::directive::{ImageSpec, NoiseRef};
use crate::gl_program::{self, ProgramError};
use crate::multipass::ShaderFiles;
use crate::source::Source;
use crate::texture::{self, Texture};

/// 1 フレームの GPU 時間の上限。Lua の毎フレームの制限と同じ値。
pub const FRAME_GPU_LIMIT: Duration = Duration::from_millis(250);

/// 拡張子でシェーダーの壁紙かどうかを決める。
pub fn is_shader_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("glsl" | "frag")
    )
}

/// 壁紙のシェーダーの前に付けるもの。`#line` でエラーの行番号をファイルの行に合わせる（`wrap` を参照）。
/// sampler の既定の精度は lowp なので、Shadertoy と同じく highp にする（RGBA32F のバッファーを
/// 16 ビットの精度で読まないように。#57）。
/// `iChannel0`〜`3` の宣言は、チャンネルの型に合わせて `prefix` で入れる。
const PREFIX_HEAD: &str = "#version 300 es
precision highp float;
precision highp int;
precision highp sampler2D;
";

const PREFIX_UNIFORMS: &str = "uniform vec3 iResolution;
uniform float iTime;
uniform float iTimeDelta;
uniform int iFrame;
uniform vec4 iMouse;
uniform vec4 iDate;
";

const PREFIX_TAIL: &str = "uniform vec3 iChannelResolution[4];
out vec4 kagegami_FragColor;
";

/// チャンネルの sampler の型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dim {
    /// `sampler2D`（画像・バッファー・何も指定していないチャンネル）。
    D2,
    /// `sampler3D`（組み込みの 3D ノイズ）。
    D3,
    /// `samplerCube`（キューブマップ）。
    Cube,
}

/// 壁紙のシェーダーの前に付けるもの。3D とキューブマップのチャンネルがなければ、ノイズを足す前と
/// 同じ文字列になる。
fn prefix(dims: [Dim; 4]) -> String {
    let mut s = String::from(PREFIX_HEAD);
    // sampler3D には既定の精度がないので、使うときだけ宣言する。
    if dims.contains(&Dim::D3) {
        s.push_str("precision highp sampler3D;\n");
    }
    // samplerCube の既定の精度は lowp なので、sampler2D と同じく highp にする。
    if dims.contains(&Dim::Cube) {
        s.push_str("precision highp samplerCube;\n");
    }
    s.push_str(PREFIX_UNIFORMS);
    for (i, d) in dims.iter().enumerate() {
        let ty = match d {
            Dim::D2 => "sampler2D",
            Dim::D3 => "sampler3D",
            Dim::Cube => "samplerCube",
        };
        s.push_str(&format!("uniform {ty} iChannel{i};\n"));
    }
    s.push_str(PREFIX_TAIL);
    s
}

/// Image のシェーダーの後に付けるもの。アルファは 1 に固定する（render.rs と同じく、
/// アルファが 1 未満だとコンポジターが半透明として扱い、背面が透けるため）。
const IMAGE_SUFFIX: &str = "
void main() {
    vec4 kagegami_c = vec4(0.0, 0.0, 0.0, 1.0);
    mainImage(kagegami_c, gl_FragCoord.xy);
    kagegami_FragColor = vec4(kagegami_c.rgb, 1.0);
}
";

/// バッファーのシェーダーの後に付けるもの。アルファも含めて、書いた値をそのまま残す。
const BUFFER_SUFFIX: &str = "
void main() {
    vec4 kagegami_c = vec4(0.0, 0.0, 0.0, 1.0);
    mainImage(kagegami_c, gl_FragCoord.xy);
    kagegami_FragColor = kagegami_c;
}
";

/// Cubemap A のシェーダーの後に付けるもの（#72）。面ごとに `kagegami_face` を変えて描く。
/// `fragCoord` は面の画素、`rayOri` は原点（Shadertoy で確かめた値）、`rayDir` は、その画素を
/// `texture(samplerCube, rayDir)` で読む向きの単位ベクトル。アルファも含めて、書いた値をそのまま残す。
const CUBEMAP_SUFFIX: &str = concat!(
    "
uniform int kagegami_face;
uniform float kagegami_size;
",
    crate::texture::cube_face_dir!(),
    "void main() {
    vec4 kagegami_c = vec4(0.0, 0.0, 0.0, 1.0);
    vec2 kagegami_st = gl_FragCoord.xy / kagegami_size * 2.0 - 1.0;
    vec3 kagegami_dir = normalize(kagegami_face_dir(kagegami_face, kagegami_st));
    mainCubemap(kagegami_c, gl_FragCoord.xy, vec3(0.0), kagegami_dir);
    kagegami_FragColor = kagegami_c;
}
"
);

/// どのパスのシェーダーか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassKind {
    Image,
    Buffer,
    /// Cubemap A（#72）。
    Cubemap,
}

/// 全画面を覆う三角形 1 つ。頂点属性を使わず gl_VertexID から作る。
const VERT: &str = "#version 300 es
void main() {
    vec2 p = vec2(float((gl_VertexID & 1) << 2), float((gl_VertexID & 2) << 1)) - 1.0;
    gl_Position = vec4(p, 0.0, 1.0);
}
";

/// 壁紙のシェーダーを GLSL ES 3.00 で包む。Shadertoy と同じく、uniform の宣言の後に Common、
/// その後にパスのソースを置く。`#line 行 ソース番号` で、エラーの位置をファイルの行に合わせる。
/// パスのソースは番号 0、Common は番号 1 にする（Mesa のエラーは `番号:行(列)` の形）。
pub fn wrap(common: Option<&str>, src: &str, kind: PassKind, dims: [Dim; 4]) -> String {
    let suffix = match kind {
        PassKind::Image => IMAGE_SUFFIX,
        PassKind::Buffer => BUFFER_SUFFIX,
        PassKind::Cubemap => CUBEMAP_SUFFIX,
    };
    let prefix = prefix(dims);
    match common {
        Some(c) => format!("{prefix}#line 1 1\n{c}\n#line 1 0\n{src}{suffix}"),
        None => format!("{prefix}#line 1\n{src}{suffix}"),
    }
}

/// `iDate`: (年, 月 - 1, 日, 0 時からの秒数)。ローカル時刻。
pub fn local_date() -> [f32; 4] {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let t = now.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return [0.0; 4];
    }
    let secs = (tm.tm_hour * 3600 + tm.tm_min * 60 + tm.tm_sec) as f64
        + f64::from(now.subsec_nanos()) / 1e9;
    [
        (tm.tm_year + 1900) as f32,
        tm.tm_mon as f32,
        tm.tm_mday as f32,
        secs as f32,
    ]
}

/// `iMouse` をバッファーのピクセルに換算する（#25、#28）。シェーダーの解像度もバッファーのピクセルにする。
/// `mouse` は `Drag::imouse` の値（出力のピクセル、左下原点）。
/// バッファーのサイズは四捨五入しているので、縦横それぞれの比で換算する。
pub fn mouse_to_buffer_px(
    buffer_size: (u32, u32),
    output_size: (u32, u32),
    mouse: [f32; 4],
) -> [f32; 4] {
    let sx = |v: f32| v * buffer_size.0 as f32 / output_size.0 as f32;
    let sy = |v: f32| v * buffer_size.1 as f32 / output_size.1 as f32;
    let [x, y, z, w] = mouse;
    [sx(x), sy(y), sx(z), sy(w)]
}

pub struct Uniforms {
    /// 描画するバッファーのピクセルサイズ。`iResolution` と viewport に使う。
    pub resolution: (u32, u32),
    pub time: f32,
    pub time_delta: f32,
    pub frame: i32,
    pub mouse: [f32; 4],
    pub date: [f32; 4],
}

/// `iChannel0`〜`3` に渡す画像・キューブマップ・3D ノイズ（Cubemap A のパスは含まない）。どれも指定していないチャンネルは
/// 黒い 1×1 のテクスチャーにする。
pub struct Channels {
    images: [Option<Texture>; 4],
    black: Texture,
}

impl Channels {
    /// 指定された画像をデコードしてテクスチャーにし、ノイズのテクスチャーを作る。
    /// デコードは画像ごとに別のスレッドで並べて行う。
    /// どれか 1 つでも読めなければ、作ったテクスチャーを消してエラーを返す。
    /// エラーは `<壁紙のファイル>:<行>: <画像のパス>: <内容>` の形。
    pub fn load(
        gl: &glow::Context,
        source: &Source,
        specs: &[ImageSpec],
        noise: &[NoiseRef],
        wallpaper: &Path,
    ) -> Result<Self, String> {
        let max = texture::max_size(gl);
        let decoded = crate::image::load_all(source, specs, wallpaper, max);
        let mut images: [Option<Texture>; 4] = Default::default();
        let mut result = Ok(());
        for (spec, img) in specs.iter().zip(decoded) {
            let tex = img.and_then(|img| {
                if spec.cubemap {
                    let face = texture::cubemap_face_size(
                        img.width,
                        img.height,
                        texture::max_cube_size(gl),
                    )
                    .map_err(|e| format!("{}: {e}", spec.resolve(wallpaper).display()))?;
                    texture::upload_cubemap(gl, &img, face, spec.sampler)
                } else {
                    texture::upload(gl, &img, spec.sampler)
                }
            });
            match tex {
                Ok(t) => images[spec.name.parse::<usize>().unwrap_or(0)] = Some(t),
                Err(e) => {
                    result = Err(format!("{}:{}: {e}", wallpaper.display(), spec.line));
                    break;
                }
            }
        }
        if result.is_ok() {
            for r in noise {
                match texture::upload_noise(gl, r.noise, r.sampler) {
                    Ok(t) => images[r.channel] = Some(t),
                    Err(e) => {
                        result = Err(format!("{}:{}: {e}", wallpaper.display(), r.line));
                        break;
                    }
                }
            }
        }
        let black = result.and_then(|()| texture::black(gl));
        match black {
            Ok(black) => Ok(Self { images, black }),
            Err(e) => {
                for t in images.into_iter().flatten() {
                    texture::delete(gl, t);
                }
                Err(e)
            }
        }
    }

    /// `iChannelResolution`。画像は (幅, 高さ, 1)、キューブマップは (面のサイズ, 面のサイズ, 1)、
    /// ノイズは (32, 32, 32)。どれも指定していないチャンネルは (0, 0, 0)。
    pub fn resolutions(&self) -> [f32; 12] {
        let mut r = [0.0; 12];
        for (i, t) in self.images.iter().enumerate() {
            if let Some(t) = t {
                r[i * 3] = t.width as f32;
                r[i * 3 + 1] = t.height as f32;
                r[i * 3 + 2] = t.depth as f32;
            }
        }
        r
    }

    /// (ターゲット, テクスチャー)。
    pub fn texture(&self, i: usize) -> (u32, glow::Texture) {
        let t = self.images[i].as_ref().unwrap_or(&self.black);
        (t.target, t.tex)
    }

    pub fn delete(self, gl: &glow::Context) {
        for t in self.images.into_iter().flatten() {
            texture::delete(gl, t);
        }
        texture::delete(gl, self.black);
    }
}

/// すべてのパスの画像。読み直すまで、すべての出力で同じものを使う。
pub struct ShaderImages {
    pub image: Channels,
    /// 添字はバッファーの添字。
    pub buffers: [Option<Channels>; 4],
    /// Cubemap A のパスの画像（#72）。
    pub cubemap: Option<Channels>,
}

impl ShaderImages {
    /// 画像はそれぞれ、指定を書いたファイルのディレクトリから探す。
    pub fn load(gl: &glow::Context, source: &Source, files: &ShaderFiles) -> Result<Self, String> {
        let image = Channels::load(
            gl,
            source,
            files.image.images(),
            files.image.noise_refs(),
            &files.image.path,
        )?;
        let mut images = Self {
            image,
            buffers: Default::default(),
            cubemap: None,
        };
        let load = |pass: &crate::multipass::PassFile| {
            Channels::load(gl, source, pass.images(), pass.noise_refs(), &pass.path)
        };
        for (i, pass) in files.buffers.iter().enumerate() {
            let Some(pass) = pass else { continue };
            match load(pass) {
                Ok(ch) => images.buffers[i] = Some(ch),
                Err(e) => {
                    images.delete(gl);
                    return Err(e);
                }
            }
        }
        if let Some(pass) = &files.cubemap {
            match load(pass) {
                Ok(ch) => images.cubemap = Some(ch),
                Err(e) => {
                    images.delete(gl);
                    return Err(e);
                }
            }
        }
        Ok(images)
    }

    pub fn delete(self, gl: &glow::Context) {
        self.image.delete(gl);
        for ch in self.buffers.into_iter().chain([self.cubemap]).flatten() {
            ch.delete(gl);
        }
    }
}

/// 1 回の描画でチャンネルに渡すもの。
pub struct Bindings {
    /// (ターゲット, テクスチャー)。ノイズのチャンネルは `TEXTURE_3D`、キューブマップは `TEXTURE_CUBE_MAP`。
    pub textures: [(u32, glow::Texture); 4],
    /// バッファーのチャンネルだけ Some（画像はテクスチャーの設定で読む）。
    pub samplers: [Option<glow::Sampler>; 4],
    /// `iChannelResolution`。
    pub resolutions: [f32; 12],
}

impl Bindings {
    pub fn images(channels: &Channels) -> Self {
        Self {
            textures: [0, 1, 2, 3].map(|i| channels.texture(i)),
            samplers: [None; 4],
            resolutions: channels.resolutions(),
        }
    }

    /// チャンネル `i` をキーボードの入力のテクスチャーにする（#69）。
    pub fn set_keyboard(&mut self, i: usize, t: &crate::texture::Texture) {
        self.textures[i] = (glow::TEXTURE_2D, t.tex);
        self.samplers[i] = None;
        self.resolutions[i * 3] = t.width as f32;
        self.resolutions[i * 3 + 1] = t.height as f32;
        self.resolutions[i * 3 + 2] = 1.0;
    }

    /// チャンネル `i` を Cubemap A のテクスチャーにする（#72）。`iChannelResolution` は (面のサイズ, 面のサイズ, 1)。
    pub fn set_cubemap(&mut self, i: usize, tex: glow::Texture, sampler: glow::Sampler, size: u32) {
        self.textures[i] = (glow::TEXTURE_CUBE_MAP, tex);
        self.samplers[i] = Some(sampler);
        self.resolutions[i * 3] = size as f32;
        self.resolutions[i * 3 + 1] = size as f32;
        self.resolutions[i * 3 + 2] = 1.0;
    }

    /// チャンネル `i` をバッファーのテクスチャーにする。
    pub fn set_buffer(
        &mut self,
        i: usize,
        tex: glow::Texture,
        sampler: glow::Sampler,
        size: (u32, u32),
    ) {
        self.textures[i] = (glow::TEXTURE_2D, tex);
        self.samplers[i] = Some(sampler);
        self.resolutions[i * 3] = size.0 as f32;
        self.resolutions[i * 3 + 1] = size.1 as f32;
        self.resolutions[i * 3 + 2] = 1.0;
    }
}

pub struct ShaderProgram {
    program: glow::Program,
    vao: glow::VertexArray,
    loc: [Option<glow::UniformLocation>; 7],
    /// Cubemap A のパスの `kagegami_face`・`kagegami_size`。ほかのパスでは None。
    face_loc: [Option<glow::UniformLocation>; 2],
}

impl ShaderProgram {
    /// コンパイルしてリンクする。エラーはドライバーのメッセージをそのまま返す。
    pub fn new(
        gl: &glow::Context,
        common: Option<&str>,
        src: &str,
        kind: PassKind,
        dims: [Dim; 4],
    ) -> Result<Self, String> {
        let frag = wrap(common, src, kind, dims);
        let program = gl_program::build(
            gl,
            &[
                (glow::VERTEX_SHADER, VERT),
                (glow::FRAGMENT_SHADER, frag.as_str()),
            ],
            |_| {},
        )
        .map_err(|e| match e {
            ProgramError::Create(e) => e,
            ProgramError::Compile { shader_type, log } => {
                let what = if shader_type == glow::VERTEX_SHADER {
                    "vertex shader"
                } else {
                    "shader"
                };
                format!("{what} compile error:\n{}", log.trim_end())
            }
            ProgramError::Link(log) => format!("shader link error:\n{}", log.trim_end()),
        })?;
        unsafe {
            let vao = gl.create_vertex_array()?;
            let loc = [
                "iResolution",
                "iTime",
                "iTimeDelta",
                "iFrame",
                "iMouse",
                "iDate",
                "iChannelResolution[0]",
            ]
            .map(|n| gl.get_uniform_location(program, n));
            let face_loc =
                ["kagegami_face", "kagegami_size"].map(|n| gl.get_uniform_location(program, n));
            // sampler の uniform は既定ですべてユニット 0 を指すので、チャンネルの番号のユニットにする。
            gl.use_program(Some(program));
            for i in 0..4 {
                let l = gl.get_uniform_location(program, &format!("iChannel{i}"));
                gl.uniform_1_i32(l.as_ref(), i);
            }
            Ok(Self {
                program,
                vao,
                loc,
                face_loc,
            })
        }
    }

    /// 今の framebuffer に描く。viewport は `u.resolution`。
    pub fn draw(&self, gl: &glow::Context, u: &Uniforms, channels: &Bindings) {
        unsafe {
            gl.viewport(0, 0, u.resolution.0 as i32, u.resolution.1 as i32);
            self.set_up(gl, u, channels);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            self.finish(gl);
        }
    }

    /// Cubemap A のパスを、`cube` の読まない方の 6 面に描き、描いた方を読む方にする（#72）。
    /// viewport は面のサイズで、`iResolution` は `u.resolution`（出力のサイズ。Shadertoy と同じ）のまま。
    pub fn draw_cubemap(
        &self,
        gl: &glow::Context,
        u: &Uniforms,
        channels: &Bindings,
        cube: &mut CubemapPass,
    ) {
        let [face_loc, size_loc] = &self.face_loc;
        let size = cube.size();
        unsafe {
            gl.viewport(0, 0, size as i32, size as i32);
            self.set_up(gl, u, channels);
            gl.uniform_1_f32(size_loc.as_ref(), size as f32);
            for face in 0..6 {
                cube.begin_face(gl, face);
                gl.uniform_1_i32(face_loc.as_ref(), face as i32);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            self.finish(gl);
        }
        cube.end(gl);
    }

    /// uniform とチャンネルを設定し、描く準備をする。
    unsafe fn set_up(&self, gl: &glow::Context, u: &Uniforms, channels: &Bindings) {
        let [res, time, dt, frame, mouse, date, channel_res] = &self.loc;
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_3_f32(
                res.as_ref(),
                u.resolution.0 as f32,
                u.resolution.1 as f32,
                1.0,
            );
            gl.uniform_1_f32(time.as_ref(), u.time);
            gl.uniform_1_f32(dt.as_ref(), u.time_delta);
            gl.uniform_1_i32(frame.as_ref(), u.frame);
            let [mx, my, mz, mw] = u.mouse;
            gl.uniform_4_f32(mouse.as_ref(), mx, my, mz, mw);
            let [dy, dm, dd, ds] = u.date;
            gl.uniform_4_f32(date.as_ref(), dy, dm, dd, ds);
            gl.uniform_3_f32_slice(channel_res.as_ref(), &channels.resolutions);
            for i in 0..4 {
                gl.active_texture(glow::TEXTURE0 + i as u32);
                let (target, tex) = channels.textures[i];
                gl.bind_texture(target, Some(tex));
                gl.bind_sampler(i as u32, channels.samplers[i]);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.disable(glow::BLEND);
            // 図形の描画が使う既定の VAO の設定を壊さないように、専用の空の VAO で描く。
            gl.bind_vertex_array(Some(self.vao));
        }
    }

    /// `set_up` で変えた状態を戻す。
    unsafe fn finish(&self, gl: &glow::Context) {
        unsafe {
            gl.bind_vertex_array(None);
            for i in 0..4 {
                gl.bind_sampler(i, None);
            }
            gl.enable(glow::BLEND);
        }
    }

    pub fn delete(self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
        }
    }
}

/// EXT_disjoint_timer_query の定数（glow には GPU_DISJOINT_EXT がない）。
const GPU_DISJOINT_EXT: u32 = 0x8FBB;
/// 結果を待つクエリーの数。結果は数フレーム遅れて届くので、順番に使い回す。
const QUERIES: usize = 3;

type GetQueryObjectUi64v = unsafe extern "system" fn(u32, u32, *mut u64);

/// `GpuTimer::poll` で届いた、1 フレームごとの GPU 時間。
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct GpuTimes {
    times: [Duration; QUERIES],
    len: usize,
}

impl GpuTimes {
    fn push(&mut self, d: Duration) {
        self.times[self.len] = d;
        self.len += 1;
    }

    pub fn as_slice(&self) -> &[Duration] {
        &self.times[..self.len]
    }

    /// 最も長い GPU 時間。届いた結果がなければ None。
    pub fn longest(&self) -> Option<Duration> {
        self.as_slice().iter().copied().max()
    }
}

/// 1 フレームの GPU 時間を測る。結果は描いた次のフレーム以降に届く。
pub struct GpuTimer {
    queries: [glow::Query; QUERIES],
    in_flight: [bool; QUERIES],
    next: usize,
    running: bool,
    get_u64: GetQueryObjectUi64v,
}

impl GpuTimer {
    /// 拡張機能がなければ None。
    pub fn new(
        gl: &glow::Context,
        proc_address: impl Fn(&str) -> *const std::ffi::c_void,
    ) -> Option<Self> {
        if !gl
            .supported_extensions()
            .contains("GL_EXT_disjoint_timer_query")
        {
            return None;
        }
        // GLES では 64 ビットの結果を読む関数は拡張機能の名前でしか取れない。
        let f = proc_address("glGetQueryObjectui64vEXT");
        if f.is_null() {
            return None;
        }
        let get_u64 =
            unsafe { std::mem::transmute::<*const std::ffi::c_void, GetQueryObjectUi64v>(f) };
        let mut queries = Vec::with_capacity(QUERIES);
        for _ in 0..QUERIES {
            queries.push(unsafe { gl.create_query() }.ok()?);
        }
        unsafe {
            // 溜まっている disjoint の印を消しておく。
            gl.get_parameter_i32(GPU_DISJOINT_EXT);
        }
        Some(Self {
            queries: queries.try_into().ok()?,
            in_flight: [false; QUERIES],
            next: 0,
            running: false,
            get_u64,
        })
    }

    /// 測っている途中の結果を捨てる（シェーダーを読み直したとき、前のシェーダーの結果で止めないため）。
    pub fn reset(&mut self) {
        self.in_flight = [false; QUERIES];
    }

    /// 届いた結果。disjoint が起きていたら、その間の結果は捨てる。
    pub fn poll(&mut self, gl: &glow::Context) -> GpuTimes {
        let mut times = GpuTimes::default();
        for i in 0..QUERIES {
            if !self.in_flight[i] {
                continue;
            }
            let q = self.queries[i];
            let available =
                unsafe { gl.get_query_parameter_u32(q, glow::QUERY_RESULT_AVAILABLE) } != 0;
            if !available {
                continue;
            }
            let mut ns = 0u64;
            unsafe { (self.get_u64)(q.0.get(), glow::QUERY_RESULT, &mut ns) };
            self.in_flight[i] = false;
            times.push(Duration::from_nanos(ns));
        }
        let disjoint = unsafe { gl.get_parameter_i32(GPU_DISJOINT_EXT) } != 0;
        if disjoint { GpuTimes::default() } else { times }
    }

    /// 測り始める。空いているクエリーがなければ、このフレームは測らない。
    pub fn begin(&mut self, gl: &glow::Context) {
        let i = self.next;
        if self.in_flight[i] {
            return;
        }
        unsafe { gl.begin_query(glow::TIME_ELAPSED, self.queries[i]) };
        self.running = true;
    }

    pub fn delete(self, gl: &glow::Context) {
        for q in self.queries {
            unsafe { gl.delete_query(q) };
        }
    }

    pub fn end(&mut self, gl: &glow::Context) {
        if !self.running {
            return;
        }
        unsafe { gl.end_query(glow::TIME_ELAPSED) };
        self.running = false;
        self.in_flight[self.next] = true;
        self.next = (self.next + 1) % QUERIES;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_times_keep_every_result() {
        let mut t = GpuTimes::default();
        assert_eq!(t.longest(), None);
        t.push(Duration::from_millis(2));
        t.push(Duration::from_millis(5));
        t.push(Duration::from_millis(3));
        assert_eq!(t.as_slice().len(), 3);
        assert_eq!(t.longest(), Some(Duration::from_millis(5)));
    }

    #[test]
    fn picks_shader_by_extension() {
        assert!(is_shader_path(Path::new("a.glsl")));
        assert!(is_shader_path(Path::new("dir/a.frag")));
        assert!(!is_shader_path(Path::new("a.lua")));
        assert!(!is_shader_path(Path::new("glsl")));
    }

    #[test]
    fn wrap_puts_version_first_and_resets_line() {
        let s = wrap(
            None,
            "void mainImage(out vec4 c, in vec2 p) { c = vec4(1.0); }",
            PassKind::Image,
            [Dim::D2; 4],
        );
        assert!(s.starts_with("#version 300 es\n"));
        assert!(s.contains("precision highp sampler2D;\n"));
        assert!(s.contains("uniform int iFrame;"));
        assert!(s.contains("uniform sampler2D iChannel3;"));
        assert!(s.contains("uniform vec3 iChannelResolution[4];"));
        assert!(s.contains("#line 1\nvoid mainImage"));
        assert!(s.contains("vec4(kagegami_c.rgb, 1.0)"));
    }

    #[test]
    fn prefix_without_noise_is_unchanged() {
        // ノイズを足す前（#70 より前）の前置き。ノイズを使わない壁紙では同じ文字列にする。
        const BEFORE: &str = "#version 300 es
precision highp float;
precision highp int;
precision highp sampler2D;
uniform vec3 iResolution;
uniform float iTime;
uniform float iTimeDelta;
uniform int iFrame;
uniform vec4 iMouse;
uniform vec4 iDate;
uniform sampler2D iChannel0;
uniform sampler2D iChannel1;
uniform sampler2D iChannel2;
uniform sampler2D iChannel3;
uniform vec3 iChannelResolution[4];
out vec4 kagegami_FragColor;
";
        assert_eq!(prefix([Dim::D2; 4]), BEFORE);
    }

    #[test]
    fn noise_channels_are_sampler3d() {
        let s = prefix([Dim::D2, Dim::D3, Dim::D2, Dim::D3]);
        assert!(s.contains("precision highp sampler3D;\n"));
        assert!(s.contains("uniform sampler2D iChannel0;\n"));
        assert!(s.contains("uniform sampler3D iChannel1;\n"));
        assert!(s.contains("uniform sampler2D iChannel2;\n"));
        assert!(s.contains("uniform sampler3D iChannel3;\n"));
        // 精度の宣言は、使う前に置く。
        assert!(s.find("sampler3D;").unwrap() < s.find("uniform sampler3D").unwrap());
    }

    #[test]
    fn cubemap_channels_are_sampler_cube() {
        let s = prefix([Dim::Cube, Dim::D2, Dim::D3, Dim::D2]);
        assert!(s.contains("precision highp samplerCube;\n"));
        assert!(s.contains("uniform samplerCube iChannel0;\n"));
        assert!(s.contains("uniform sampler2D iChannel1;\n"));
        assert!(s.contains("uniform sampler3D iChannel2;\n"));
        assert!(s.find("samplerCube;").unwrap() < s.find("uniform samplerCube").unwrap());
        // キューブマップを使わなければ、精度の宣言も足さない。
        assert!(!prefix([Dim::D2; 4]).contains("samplerCube"));
    }

    #[test]
    fn cubemap_pass_calls_main_cubemap_after_the_same_prefix() {
        let src = "void mainCubemap(out vec4 c, in vec2 p, in vec3 o, in vec3 d) {}";
        let dims = [Dim::Cube, Dim::D2, Dim::D2, Dim::D2];
        let s = wrap(None, src, PassKind::Cubemap, dims);
        // 前置きはほかのパスと同じで、面ごとの uniform はパスのソースの後ろに置く。
        assert!(s.starts_with(&format!("{}#line 1\n{src}", prefix(dims))));
        assert!(s.contains("mainCubemap(kagegami_c, gl_FragCoord.xy, vec3(0.0), kagegami_dir);"));
        assert!(s.contains("kagegami_FragColor = kagegami_c;"));
        assert!(!wrap(None, src, PassKind::Buffer, dims).contains("kagegami_face"));
    }

    #[test]
    fn wrap_puts_common_between_uniforms_and_the_pass() {
        let s = wrap(
            Some("float k() { return 1.0; }"),
            "void mainImage(out vec4 c, in vec2 p) {}",
            PassKind::Buffer,
            [Dim::D2; 4],
        );
        let uniforms = s.find("uniform vec3 iChannelResolution[4];").unwrap();
        let common = s.find("#line 1 1\nfloat k()").unwrap();
        let pass = s.find("#line 1 0\nvoid mainImage").unwrap();
        assert!(uniforms < common && common < pass);
        // バッファーはアルファも含めて書いた値を残す。
        assert!(s.contains("kagegami_FragColor = kagegami_c;"));
        assert!(!s.contains("kagegami_c.rgb, 1.0"));
    }

    #[test]
    fn every_shader_example_names_files_and_images_that_load() {
        // examples/*.glsl と、マルチパスの例のディレクトリ examples/<名前>/（入口は main.glsl）。
        let mut args = Vec::new();
        for entry in std::fs::read_dir("examples").unwrap() {
            let path = entry.unwrap().path();
            if (path.is_dir() && path.join("main.glsl").exists()) || is_shader_path(&path) {
                args.push(path);
            }
        }
        assert!(args.iter().any(|p| p.is_dir()));
        for arg in args {
            let source = Source::open(&arg).unwrap_or_else(|e| panic!("{e}"));
            let files = ShaderFiles::read(&source, source.entry(), &mut vec![])
                .unwrap_or_else(|e| panic!("{}: {e}", arg.display()));
            for pass in files.passes() {
                for spec in pass.images() {
                    crate::image::load(&source, &spec.resolve(&pass.path), 2048)
                        .unwrap_or_else(|e| panic!("{}: {e}", pass.path.display()));
                }
            }
        }
    }

    /// `type=cubemap` の壁紙を、アプリと同じ経路（指定の読み取り → コンパイル → 画像の読み込み →
    /// 描画）で描くと、#68 の向きの式で元の画像に戻る。縦横比が 2:1 でない画像はエラーになる。
    #[test]
    fn cubemap_wallpaper_draws_the_panorama() {
        let Some((_egl, gl)) = crate::gpu_test::context() else {
            eprintln!("skipped: no surfaceless EGL display");
            return;
        };
        let dir = std::env::temp_dir().join(format!("kagegami-cubemap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = |name: &str, w: u32, h: u32, rgba: &[u8]| {
            let mut out = Vec::new();
            let mut e = png::Encoder::new(&mut out, w, h);
            e.set_color(png::ColorType::Rgba);
            e.write_header().unwrap().write_image_data(rgba).unwrap();
            std::fs::write(dir.join(name), out).unwrap();
        };
        let (w, h) = (256u32, 128u32);
        let rgba = crate::gpu_test::panorama(w, h);
        png("pano.png", w, h, &rgba);
        png("bad.png", w, 100, &rgba[..(w * 100 * 4) as usize]);
        std::fs::write(
            dir.join("main.glsl"),
            "// @iChannel0 pano.png type=cubemap filter=linear
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float lon = (uv.x - 0.5) * 6.2831853;
    float lat = (uv.y - 0.5) * 3.1415927;
    vec3 dir = vec3(sin(lon) * cos(lat), sin(lat), -cos(lon) * cos(lat));
    fragColor = texture(iChannel0, dir);
}
",
        )
        .unwrap();
        std::fs::write(
            dir.join("bad.glsl"),
            "// @iChannel1 bad.png type=cubemap
void mainImage(out vec4 c, in vec2 p) { c = texture(iChannel1, vec3(p, 1.0)); }
",
        )
        .unwrap();

        let source = Source::open(&dir.join("main.glsl")).unwrap();
        let files = ShaderFiles::read(&source, source.entry(), &mut vec![]).unwrap();
        let program = ShaderProgram::new(
            &gl,
            None,
            &files.image.src,
            PassKind::Image,
            files.image.channel_dims(),
        )
        .unwrap();
        let images = ShaderImages::load(&gl, &source, &files).unwrap();
        let bindings = Bindings::images(&images.image);
        // iChannelResolution は (面のサイズ, 面のサイズ, 1)。
        assert_eq!(bindings.resolutions[..3], [64.0, 64.0, 1.0]);
        assert_eq!(bindings.textures[0].0, glow::TEXTURE_CUBE_MAP);
        let u = Uniforms {
            resolution: (w, h),
            time: 0.0,
            time_delta: 0.0,
            frame: 0,
            mouse: [0.0; 4],
            date: [0.0; 4],
        };
        let out = crate::gpu_test::render(&gl, (w, h), || program.draw(&gl, &u, &bindings));
        crate::gpu_test::assert_unwrapped(&gl, &out, &rgba, (w, h));
        program.delete(&gl);
        images.delete(&gl);

        let source = Source::open(&dir.join("bad.glsl")).unwrap();
        let files = ShaderFiles::read(&source, source.entry(), &mut vec![]).unwrap();
        let e = match ShaderImages::load(&gl, &source, &files) {
            Ok(_) => panic!("bad.png was loaded"),
            Err(e) => e,
        };
        assert!(e.contains("bad.glsl:1: "), "{e}");
        assert!(
            e.ends_with("bad.png: a cubemap must be an equirectangular image with a 2:1 aspect ratio (got 256x100)"),
            "{e}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn date_is_plausible() {
        let [y, m, d, s] = local_date();
        assert!(y >= 2024.0);
        assert!((0.0..12.0).contains(&m));
        assert!((1.0..=31.0).contains(&d));
        assert!((0.0..86401.0).contains(&s));
    }

    #[test]
    fn mouse_is_converted_to_buffer_pixels() {
        // 等倍では出力のピクセルと同じ。
        assert_eq!(
            mouse_to_buffer_px((1920, 1080), (1920, 1080), [100.0, 80.0, 100.0, -80.0]),
            [100.0, 80.0, 100.0, -80.0]
        );
        // -s 0.5: iMouse もバッファーのピクセルにする。符号は変えない。
        assert_eq!(
            mouse_to_buffer_px((960, 540), (1920, 1080), [100.0, 80.0, -100.0, -80.0]),
            [50.0, 40.0, -50.0, -40.0]
        );
        // -s 2
        assert_eq!(
            mouse_to_buffer_px((3840, 2160), (1920, 1080), [100.0, 80.0, 100.0, 80.0]),
            [200.0, 160.0, 200.0, 160.0]
        );
        // 四捨五入したバッファーでは、縦横それぞれの比で換算する（1366 * 0.25 = 341.5 -> 342）。
        assert_eq!(
            mouse_to_buffer_px((342, 192), (1366, 768), [1366.0, 768.0, 0.0, 0.0]),
            [342.0, 192.0, 0.0, 0.0]
        );
        // 一度も押していなければ全部 0 のまま。
        assert_eq!(
            mouse_to_buffer_px((960, 540), (1920, 1080), [0.0; 4]),
            [0.0; 4]
        );
    }
}

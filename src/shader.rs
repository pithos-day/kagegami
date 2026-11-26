//! Shadertoy 形式のシェーダーの壁紙（#11）。
//!
//! `mainImage(out vec4, in vec2)` を書いたファイルを GLSL ES 3.00 で包み、全画面に描く。
//! 1 フレームの GPU 時間を `EXT_disjoint_timer_query` で測り、長すぎたらエラーとして止める。
//! バッファー（Buffer A〜D）と Common を使う壁紙（#57）は、パスごとに 1 つのプログラムにする。

use std::path::Path;
use std::time::Duration;

use glow::HasContext;

use crate::directive::ImageSpec;
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
const PREFIX: &str = "#version 300 es
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

/// どのパスのシェーダーか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassKind {
    Image,
    Buffer,
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
pub fn wrap(common: Option<&str>, src: &str, kind: PassKind) -> String {
    let suffix = match kind {
        PassKind::Image => IMAGE_SUFFIX,
        PassKind::Buffer => BUFFER_SUFFIX,
    };
    match common {
        Some(c) => format!("{PREFIX}#line 1 1\n{c}\n#line 1 0\n{src}{suffix}"),
        None => format!("{PREFIX}#line 1\n{src}{suffix}"),
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

/// `iChannel0`〜`3` に渡す画像。画像を指定していないチャンネルは黒い 1×1 のテクスチャーにする。
pub struct Channels {
    images: [Option<Texture>; 4],
    black: Texture,
}

impl Channels {
    /// 指定された画像をデコードしてテクスチャーにする。デコードは画像ごとに別のスレッドで並べて行う。
    /// どれか 1 つでも読めなければ、作ったテクスチャーを消してエラーを返す。
    /// エラーは `<壁紙のファイル>:<行>: <画像のパス>: <内容>` の形。
    pub fn load(
        gl: &glow::Context,
        source: &Source,
        specs: &[ImageSpec],
        wallpaper: &Path,
    ) -> Result<Self, String> {
        let max = texture::max_size(gl);
        let decoded = crate::image::load_all(source, specs, wallpaper, max);
        let mut images: [Option<Texture>; 4] = Default::default();
        let mut result = Ok(());
        for (spec, img) in specs.iter().zip(decoded) {
            let tex = img.and_then(|img| texture::upload(gl, &img, spec.sampler));
            match tex {
                Ok(t) => images[spec.name.parse::<usize>().unwrap_or(0)] = Some(t),
                Err(e) => {
                    result = Err(format!("{}:{}: {e}", wallpaper.display(), spec.line));
                    break;
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

    /// `iChannelResolution`。画像を指定していないチャンネルは (0, 0, 0)。
    pub fn resolutions(&self) -> [f32; 12] {
        let mut r = [0.0; 12];
        for (i, t) in self.images.iter().enumerate() {
            if let Some(t) = t {
                r[i * 3] = t.width as f32;
                r[i * 3 + 1] = t.height as f32;
                r[i * 3 + 2] = 1.0;
            }
        }
        r
    }

    pub fn texture(&self, i: usize) -> glow::Texture {
        self.images[i].as_ref().unwrap_or(&self.black).tex
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
}

impl ShaderImages {
    /// 画像はそれぞれ、指定を書いたファイルのディレクトリから探す。
    pub fn load(gl: &glow::Context, source: &Source, files: &ShaderFiles) -> Result<Self, String> {
        let image = Channels::load(gl, source, files.image.images(), &files.image.path)?;
        let mut buffers: [Option<Channels>; 4] = Default::default();
        for (i, pass) in files.buffers.iter().enumerate() {
            let Some(pass) = pass else { continue };
            match Channels::load(gl, source, pass.images(), &pass.path) {
                Ok(ch) => buffers[i] = Some(ch),
                Err(e) => {
                    Self { image, buffers }.delete(gl);
                    return Err(e);
                }
            }
        }
        Ok(Self { image, buffers })
    }

    pub fn delete(self, gl: &glow::Context) {
        self.image.delete(gl);
        for ch in self.buffers.into_iter().flatten() {
            ch.delete(gl);
        }
    }
}

/// 1 回の描画でチャンネルに渡すもの。
pub struct Bindings {
    pub textures: [glow::Texture; 4],
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
        self.textures[i] = t.tex;
        self.samplers[i] = None;
        self.resolutions[i * 3] = t.width as f32;
        self.resolutions[i * 3 + 1] = t.height as f32;
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
        self.textures[i] = tex;
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
}

impl ShaderProgram {
    /// コンパイルしてリンクする。エラーはドライバーのメッセージをそのまま返す。
    pub fn new(
        gl: &glow::Context,
        common: Option<&str>,
        src: &str,
        kind: PassKind,
    ) -> Result<Self, String> {
        let frag = wrap(common, src, kind);
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
            // sampler の uniform は既定ですべてユニット 0 を指すので、チャンネルの番号のユニットにする。
            gl.use_program(Some(program));
            for i in 0..4 {
                let l = gl.get_uniform_location(program, &format!("iChannel{i}"));
                gl.uniform_1_i32(l.as_ref(), i);
            }
            Ok(Self { program, vao, loc })
        }
    }

    pub fn draw(&self, gl: &glow::Context, u: &Uniforms, channels: &Bindings) {
        let [res, time, dt, frame, mouse, date, channel_res] = &self.loc;
        unsafe {
            gl.viewport(0, 0, u.resolution.0 as i32, u.resolution.1 as i32);
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
                gl.bind_texture(glow::TEXTURE_2D, Some(channels.textures[i]));
                gl.bind_sampler(i as u32, channels.samplers[i]);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.disable(glow::BLEND);
            // 図形の描画が使う既定の VAO の設定を壊さないように、専用の空の VAO で描く。
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
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
    fn wrap_puts_common_between_uniforms_and_the_pass() {
        let s = wrap(
            Some("float k() { return 1.0; }"),
            "void mainImage(out vec4 c, in vec2 p) {}",
            PassKind::Buffer,
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

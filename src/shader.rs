//! Shadertoy 形式のシェーダーの壁紙（#11）。
//!
//! `mainImage(out vec4, in vec2)` を書いたファイルを GLSL ES 3.00 で包み、全画面に描く。
//! 1 フレームの GPU 時間を `EXT_disjoint_timer_query` で測り、長すぎたらエラーとして止める。

use std::path::Path;
use std::time::Duration;

use glow::HasContext;

use crate::gl_program::{self, ProgramError};

/// 1 フレームの GPU 時間の上限。Lua の毎フレームの制限と同じ値。
pub const FRAME_GPU_LIMIT: Duration = Duration::from_millis(250);

/// 拡張子でシェーダーの壁紙かどうかを決める。
pub fn is_shader_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("glsl" | "frag")
    )
}

/// 壁紙のシェーダーの前に付けるもの。`#line 1` でエラーの行番号をファイルの行に合わせる。
const PREFIX: &str = "#version 300 es
precision highp float;
precision highp int;
uniform vec3 iResolution;
uniform float iTime;
uniform float iTimeDelta;
uniform int iFrame;
uniform vec4 iMouse;
uniform vec4 iDate;
out vec4 kagegami_FragColor;
#line 1
";

/// 壁紙のシェーダーの後に付けるもの。アルファは 1 に固定する（render.rs と同じく、
/// アルファが 1 未満だとコンポジターが半透明として扱い、背面が透けるため）。
const SUFFIX: &str = "
void main() {
    vec4 kagegami_c = vec4(0.0, 0.0, 0.0, 1.0);
    mainImage(kagegami_c, gl_FragCoord.xy);
    kagegami_FragColor = vec4(kagegami_c.rgb, 1.0);
}
";

/// 全画面を覆う三角形 1 つ。頂点属性を使わず gl_VertexID から作る。
const VERT: &str = "#version 300 es
void main() {
    vec2 p = vec2(float((gl_VertexID & 1) << 2), float((gl_VertexID & 2) << 1)) - 1.0;
    gl_Position = vec4(p, 0.0, 1.0);
}
";

pub fn wrap(src: &str) -> String {
    format!("{PREFIX}{src}{SUFFIX}")
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

pub struct ShaderProgram {
    program: glow::Program,
    vao: glow::VertexArray,
    loc: [Option<glow::UniformLocation>; 6],
}

impl ShaderProgram {
    /// コンパイルしてリンクする。エラーはドライバーのメッセージをそのまま返す。
    pub fn new(gl: &glow::Context, src: &str) -> Result<Self, String> {
        let frag = wrap(src);
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
            ]
            .map(|n| gl.get_uniform_location(program, n));
            Ok(Self { program, vao, loc })
        }
    }

    pub fn draw(&self, gl: &glow::Context, u: &Uniforms) {
        let [res, time, dt, frame, mouse, date] = &self.loc;
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
            gl.disable(glow::BLEND);
            // 図形の描画が使う既定の VAO の設定を壊さないように、専用の空の VAO で描く。
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
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

    /// 届いた結果のうち最も長い GPU 時間。disjoint が起きていたら、その間の結果は捨てる。
    pub fn poll(&mut self, gl: &glow::Context) -> Option<Duration> {
        let mut longest: Option<Duration> = None;
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
            let d = Duration::from_nanos(ns);
            longest = Some(longest.map_or(d, |l| l.max(d)));
        }
        let disjoint = unsafe { gl.get_parameter_i32(GPU_DISJOINT_EXT) } != 0;
        if disjoint { None } else { longest }
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
    fn picks_shader_by_extension() {
        assert!(is_shader_path(Path::new("a.glsl")));
        assert!(is_shader_path(Path::new("dir/a.frag")));
        assert!(!is_shader_path(Path::new("a.lua")));
        assert!(!is_shader_path(Path::new("glsl")));
    }

    #[test]
    fn wrap_puts_version_first_and_resets_line() {
        let s = wrap("void mainImage(out vec4 c, in vec2 p) { c = vec4(1.0); }");
        assert!(s.starts_with("#version 300 es\n"));
        assert!(s.contains("uniform int iFrame;"));
        assert!(s.contains("#line 1\nvoid mainImage"));
        assert!(s.contains("vec4(kagegami_c.rgb, 1.0)"));
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

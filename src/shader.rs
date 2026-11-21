//! Shadertoy 形式のシェーダーの壁紙（#11）。
//!
//! `mainImage(out vec4, in vec2)` を書いたファイルを GLSL ES 3.00 で包み、全画面に描く。
//! 1 フレームの GPU 時間を `EXT_disjoint_timer_query` で測り、長すぎたらエラーとして止める。

use std::path::Path;
use std::time::Duration;

use glow::HasContext;

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

/// Shadertoy の `iMouse` の状態（#11 で決めた動き）。座標は Lua の `mouse` と同じ
/// バッファーのピクセル（左上原点）で持ち、uniform にするときに y を反転する。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DragMouse {
    pos: [f32; 2],
    start: [f32; 2],
    /// ドラッグ中（z が正）。
    down: bool,
    /// 押した後、まだ 1 フレームも描いていない（w が正）。
    clicked: bool,
    /// 一度でも押したか。押す前は Shadertoy と同じく全部 0 にする。
    touched: bool,
}

impl DragMouse {
    /// どれかのボタンを押した。ほかのボタンを押していても、ここからドラッグを始め直す。
    pub fn pressed(&mut self, x: f32, y: f32) {
        self.pos = [x, y];
        self.start = [x, y];
        self.down = true;
        self.clicked = true;
        self.touched = true;
    }

    /// どれかのボタンを離した、またはカーソルが surface の外に出た（Leave）。
    pub fn released(&mut self) {
        self.down = false;
    }

    /// カーソルが動いた。ドラッグ中だけ位置を更新する（surface の外の座標もそのまま）。
    pub fn moved(&mut self, x: f32, y: f32) {
        if self.down {
            self.pos = [x, y];
        }
    }

    /// 1 フレーム描いた。押した直後の印（w が正）はここで消える。
    pub fn frame_rendered(&mut self) {
        self.clicked = false;
    }

    /// `iMouse` の値。y は `height - y` にして左下原点にする。
    pub fn uniform(&self, height: f32) -> [f32; 4] {
        if !self.touched {
            return [0.0; 4];
        }
        let sign = |b: bool| if b { 1.0 } else { -1.0 };
        [
            self.pos[0],
            height - self.pos[1],
            self.start[0] * sign(self.down),
            (height - self.start[1]) * sign(self.clicked),
        ]
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

pub struct Uniforms {
    pub width: u32,
    pub height: u32,
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
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            let mut result = Ok(());
            for (ty, s, what) in [
                (glow::VERTEX_SHADER, VERT, "vertex shader"),
                (glow::FRAGMENT_SHADER, frag.as_str(), "shader"),
            ] {
                let sh = gl.create_shader(ty)?;
                gl.shader_source(sh, s);
                gl.compile_shader(sh);
                shaders.push(sh);
                if !gl.get_shader_compile_status(sh) {
                    result = Err(format!(
                        "{what} compile error:\n{}",
                        gl.get_shader_info_log(sh).trim_end()
                    ));
                    break;
                }
                gl.attach_shader(program, sh);
            }
            if result.is_ok() {
                gl.link_program(program);
                if !gl.get_program_link_status(program) {
                    result = Err(format!(
                        "shader link error:\n{}",
                        gl.get_program_info_log(program).trim_end()
                    ));
                }
            }
            for sh in shaders {
                gl.delete_shader(sh);
            }
            if let Err(e) = result {
                gl.delete_program(program);
                return Err(e);
            }
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
            gl.viewport(0, 0, u.width as i32, u.height as i32);
            gl.use_program(Some(self.program));
            gl.uniform_3_f32(res.as_ref(), u.width as f32, u.height as f32, 1.0);
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

    const H: f32 = 1080.0;

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
    fn starts_at_zero() {
        // Shadertoy と同じく、一度も押していなければ全部 0（y を反転しない）。
        assert_eq!(DragMouse::default().uniform(H), [0.0; 4]);
    }

    #[test]
    fn press_sets_position_and_click_for_one_frame() {
        let mut m = DragMouse::default();
        m.pressed(100.0, 80.0);
        assert_eq!(m.uniform(H), [100.0, H - 80.0, 100.0, H - 80.0]);
        m.frame_rendered();
        assert_eq!(m.uniform(H), [100.0, H - 80.0, 100.0, -(H - 80.0)]);
    }

    #[test]
    fn drag_moves_xy_only() {
        let mut m = DragMouse::default();
        m.pressed(100.0, 80.0);
        m.frame_rendered();
        m.moved(150.0, 90.0);
        assert_eq!(m.uniform(H), [150.0, H - 90.0, 100.0, -(H - 80.0)]);
        // surface の外の座標もそのまま渡す。
        m.moved(-20.0, 2000.0);
        assert_eq!(m.uniform(H), [-20.0, H - 2000.0, 100.0, -(H - 80.0)]);
    }

    #[test]
    fn hover_without_button_does_not_move() {
        let mut m = DragMouse::default();
        m.moved(10.0, 10.0);
        assert_eq!(m.uniform(H), [0.0; 4]);
        m.released();
        assert_eq!(m.uniform(H), [0.0; 4]);
    }

    #[test]
    fn release_makes_z_negative_and_keeps_values() {
        let mut m = DragMouse::default();
        m.pressed(100.0, 80.0);
        m.frame_rendered();
        m.moved(150.0, 90.0);
        m.released();
        assert_eq!(m.uniform(H), [150.0, H - 90.0, -100.0, -(H - 80.0)]);
        // 切れた後は、残っているボタンを押したまま動かしても変わらない。
        m.moved(300.0, 300.0);
        assert_eq!(m.uniform(H), [150.0, H - 90.0, -100.0, -(H - 80.0)]);
    }

    #[test]
    fn second_button_restarts_drag() {
        let mut m = DragMouse::default();
        m.pressed(100.0, 80.0);
        m.frame_rendered();
        m.moved(150.0, 90.0);
        m.pressed(150.0, 90.0);
        assert_eq!(m.uniform(H), [150.0, H - 90.0, 150.0, H - 90.0]);
    }

    #[test]
    fn press_and_release_before_a_frame_still_shows_the_click() {
        let mut m = DragMouse::default();
        m.pressed(100.0, 80.0);
        m.released();
        assert_eq!(m.uniform(H), [100.0, H - 80.0, -100.0, H - 80.0]);
        m.frame_rendered();
        assert_eq!(m.uniform(H), [100.0, H - 80.0, -100.0, -(H - 80.0)]);
    }

    #[test]
    fn date_is_plausible() {
        let [y, m, d, s] = local_date();
        assert!(y >= 2024.0);
        assert!((0.0..12.0).contains(&m));
        assert!((1.0..=31.0).contains(&d));
        assert!((0.0..86401.0).contains(&s));
    }
}

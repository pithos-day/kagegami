//! 開発用: 1 フレームあたりの GPU 時間を測る（`--features gpu-bench`）。
//!
//! - `KAGEGAMI_BENCH_FRAMES`: 計測するフレーム数（既定 300、最初の 30 フレームは捨てる）
//! - `KAGEGAMI_BENCH_GLSL`: 指定すると Lua ではなく Shadertoy 形式のシェーダーを全画面に描く
//!   （shaderbg と同じ描き方との比較用）
//!
//! glFinish で GPU の完了を待ってから時間を測るので、CPU 側の準備時間も含む。

use std::time::{Duration, Instant};

use glow::HasContext;

const WARMUP: u32 = 30;

pub struct Bench {
    frames: u32,
    target: u32,
    total: Duration,
    glsl: Option<Glsl>,
    glsl_src: Option<String>,
    started: Instant,
}

struct Glsl {
    program: glow::Program,
    vbo: glow::Buffer,
}

impl Bench {
    pub fn from_env() -> Option<Self> {
        let target = std::env::var("KAGEGAMI_BENCH_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
        let glsl_src = std::env::var("KAGEGAMI_BENCH_GLSL").ok().map(|p| std::fs::read_to_string(p).expect("read glsl"));
        Some(Self { frames: 0, target, total: Duration::ZERO, glsl: None, glsl_src, started: Instant::now() })
    }

    pub fn uses_glsl(&self) -> bool {
        self.glsl_src.is_some()
    }

    pub fn start(&self, gl: &glow::Context) -> Instant {
        unsafe { gl.finish() };
        Instant::now()
    }

    /// 1 フレームの計測を終える。計測が終わったら結果を出して終了する。
    pub fn end(&mut self, gl: &glow::Context, t0: Instant, label: &str) {
        unsafe { gl.finish() };
        self.frames += 1;
        if self.frames > WARMUP {
            self.total += t0.elapsed();
        }
        if self.frames >= WARMUP + self.target {
            let avg = self.total.as_secs_f64() * 1000.0 / self.target as f64;
            println!("bench {label}: {avg:.3} ms/frame over {} frames", self.target);
            std::process::exit(0);
        }
    }

    pub fn draw_glsl(&mut self, gl: &glow::Context, width: u32, height: u32) {
        let src = self.glsl_src.as_ref().unwrap();
        let g = self.glsl.get_or_insert_with(|| unsafe { compile(gl, src) });
        let t = self.started.elapsed().as_secs_f32();
        unsafe {
            gl.use_program(Some(g.program));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(g.vbo));
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
            for i in 1..4 {
                gl.disable_vertex_attrib_array(i);
            }
            gl.disable(glow::BLEND);
            let loc = |n: &str| gl.get_uniform_location(g.program, n);
            gl.uniform_3_f32(loc("iResolution").as_ref(), width as f32, height as f32, 1.0);
            gl.uniform_1_f32(loc("iTime").as_ref(), t);
            gl.uniform_4_f32(loc("iMouse").as_ref(), 0.0, 0.0, 0.0, 0.0);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
    }
}

unsafe fn compile(gl: &glow::Context, src: &str) -> Glsl {
    // shaderbg と同じ前置きと後置き。
    let frag = format!(
        "precision highp float;\nuniform vec3 iResolution; uniform float iTime; uniform float iTimeDelta; uniform float iFrame; uniform vec4 iMouse;\n{src}\nvoid main() {{ mainImage(gl_FragColor, gl_FragCoord.xy); }}\n"
    );
    let vert = "attribute vec2 a_pos; void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }";
    unsafe {
        let program = gl.create_program().unwrap();
        for (ty, s) in [(glow::VERTEX_SHADER, vert), (glow::FRAGMENT_SHADER, frag.as_str())] {
            let sh = gl.create_shader(ty).unwrap();
            gl.shader_source(sh, s);
            gl.compile_shader(sh);
            assert!(gl.get_shader_compile_status(sh), "{}", gl.get_shader_info_log(sh));
            gl.attach_shader(program, sh);
        }
        gl.bind_attrib_location(program, 0, "a_pos");
        gl.link_program(program);
        assert!(gl.get_program_link_status(program), "{}", gl.get_program_info_log(program));
        let vbo = gl.create_buffer().unwrap();
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        let quad: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
        let bytes = std::slice::from_raw_parts(quad.as_ptr().cast::<u8>(), 32);
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
        Glsl { program, vbo }
    }
}

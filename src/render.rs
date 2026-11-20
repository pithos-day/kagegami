//! GLES2 による 2D 描画。
//!
//! 図形はすべて「カプセル（線分を太らせた形）」の SDF として、1 図形 = 1 四角形で描く。
//! 円は長さ 0 のカプセル、線は丸い端のカプセル、矩形は判定を常に内側にした四角形。
//! 頂点はまとめて 1 回の draw call で送る。

use glow::HasContext;

use crate::egl::EglContext;

/// 1 回の draw call に詰める四角形の上限（u16 インデックスに収まる数）。
pub const MAX_QUADS: usize = 8192;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    /// 画面上の位置（ピクセル、左上原点）。
    pub pos: [f32; 2],
    /// 図形のローカル座標（ピクセル）。x は線分方向、y は法線方向。
    pub local: [f32; 2],
    /// (線分の長さ, 半径)。
    pub shape: [f32; 2],
    pub color: [u8; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Alpha,
    Add,
}

/// GPU に依存しない頂点の組み立て部分（テストしやすいように分けている）。
#[derive(Default)]
pub struct Batch {
    pub vertices: Vec<Vertex>,
    pub color: [u8; 4],
}

/// 矩形を内側扱いにするための大きな半径（mediump の範囲に収まる値）。
const FILL_RADIUS: f32 = 8192.0;
/// アンチエイリアスのために図形の外側へ広げる量（ピクセル）。
const AA_PAD: f32 = 1.0;

impl Batch {
    pub fn quads(&self) -> usize {
        self.vertices.len() / 4
    }

    fn quad(&mut self, corners: [[f32; 2]; 4], locals: [[f32; 2]; 4], shape: [f32; 2]) {
        let color = self.color;
        for i in 0..4 {
            self.vertices.push(Vertex { pos: corners[i], local: locals[i], shape, color });
        }
    }

    pub fn circle(&mut self, x: f32, y: f32, r: f32) {
        let e = r.max(0.0) + AA_PAD;
        self.quad(
            [[x - e, y - e], [x + e, y - e], [x - e, y + e], [x + e, y + e]],
            [[-e, -e], [e, -e], [-e, e], [e, e]],
            [0.0, r.max(0.0)],
        );
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32) {
        let hw = (width * 0.5).max(0.0);
        let (dx, dy) = (x2 - x1, y2 - y1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-4 {
            return self.circle(x1, y1, hw);
        }
        let (ux, uy) = (dx / len, dy / len);
        let (nx, ny) = (-uy, ux);
        let e = hw + AA_PAD;
        let at = |px: f32, py: f32, s: f32, n: f32| [px + ux * s + nx * n, py + uy * s + ny * n];
        self.quad(
            [at(x1, y1, -e, -e), at(x2, y2, e, -e), at(x1, y1, -e, e), at(x2, y2, e, e)],
            [[-e, -e], [len + e, -e], [-e, e], [len + e, e]],
            [len, hw],
        );
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.quad(
            [[x, y], [x + w, y], [x, y + h], [x + w, y + h]],
            [[0.0; 2]; 4],
            [0.0, FILL_RADIUS],
        );
    }
}

pub fn color_u8(r: f32, g: f32, b: f32, a: f32) -> [u8; 4] {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [c(r), c(g), c(b), c(a)]
}

const VERT: &str = r#"
attribute vec2 a_pos;
attribute vec2 a_local;
attribute vec2 a_shape;
attribute vec4 a_color;
uniform vec2 u_res;
varying vec2 v_local;
varying vec2 v_shape;
varying vec4 v_color;
void main() {
    v_local = a_local;
    v_shape = a_shape;
    v_color = a_color;
    vec2 ndc = a_pos / u_res * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
}
"#;

const FRAG: &str = r#"
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
varying vec2 v_local;
varying vec2 v_shape;
varying vec4 v_color;
void main() {
    float dx = v_local.x - clamp(v_local.x, 0.0, v_shape.x);
    float d = length(vec2(dx, v_local.y)) - v_shape.y;
    float a = clamp(0.5 - d, 0.0, 1.0);
    gl_FragColor = vec4(v_color.rgb, v_color.a * a);
}
"#;

pub struct Renderer {
    gl: glow::Context,
    program: glow::Program,
    u_res: glow::UniformLocation,
    vbo: glow::Buffer,
    pub batch: Batch,
    blend: Blend,
}

impl Renderer {
    pub fn new(egl: &EglContext) -> Result<Self, String> {
        let gl = unsafe { glow::Context::from_loader_function(|s| egl.proc_address(s)) };
        unsafe {
            let program = gl.create_program()?;
            for (ty, src) in [(glow::VERTEX_SHADER, VERT), (glow::FRAGMENT_SHADER, FRAG)] {
                let sh = gl.create_shader(ty)?;
                gl.shader_source(sh, src);
                gl.compile_shader(sh);
                if !gl.get_shader_compile_status(sh) {
                    return Err(format!("shader compile: {}", gl.get_shader_info_log(sh)));
                }
                gl.attach_shader(program, sh);
                gl.delete_shader(sh);
            }
            for (i, name) in ["a_pos", "a_local", "a_shape", "a_color"].iter().enumerate() {
                gl.bind_attrib_location(program, i as u32, name);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(format!("shader link: {}", gl.get_program_info_log(program)));
            }
            let u_res = gl.get_uniform_location(program, "u_res").ok_or("u_res not found")?;

            // 四角形 1 つ = 頂点 4 つ = 三角形 2 つ。インデックスは固定なので最初に 1 回だけ送る。
            let mut indices = Vec::with_capacity(MAX_QUADS * 6);
            for q in 0..MAX_QUADS as u16 {
                let b = q * 4;
                indices.extend_from_slice(&[b, b + 1, b + 2, b + 2, b + 1, b + 3]);
            }
            let ibo = gl.create_buffer()?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(&indices), glow::STATIC_DRAW);

            let vbo = gl.create_buffer()?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = std::mem::size_of::<Vertex>() as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 8);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, stride, 16);
            gl.enable_vertex_attrib_array(3);
            gl.vertex_attrib_pointer_f32(3, 4, glow::UNSIGNED_BYTE, true, stride, 24);

            gl.use_program(Some(program));
            gl.enable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);

            let r = Self {
                gl,
                program,
                u_res,
                vbo,
                batch: Batch { vertices: Vec::with_capacity(MAX_QUADS * 4), color: [255; 4] },
                blend: Blend::Alpha,
            };
            r.apply_blend();
            Ok(r)
        }
    }

    fn apply_blend(&self) {
        unsafe {
            match self.blend {
                Blend::Alpha => self.gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA),
                Blend::Add => self.gl.blend_func(glow::SRC_ALPHA, glow::ONE),
            }
        }
    }

    /// 1 フレームの描画を始める。GL の状態は出力間で共有されるので毎回設定し直す。
    pub fn begin(&mut self, width: u32, height: u32) {
        unsafe {
            self.gl.viewport(0, 0, width as i32, height as i32);
            self.gl.use_program(Some(self.program));
            self.gl.uniform_2_f32(Some(&self.u_res), width as f32, height as f32);
            self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
        }
        self.batch.vertices.clear();
        self.batch.color = [255; 4];
        self.blend = Blend::Alpha;
        self.apply_blend();
    }

    pub fn clear(&mut self, r: f32, g: f32, b: f32) {
        // clear より前に積んだ図形は消えるので捨てる。
        self.batch.vertices.clear();
        unsafe {
            self.gl.clear_color(r, g, b, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    pub fn set_blend(&mut self, blend: Blend) {
        if blend != self.blend {
            self.flush();
            self.blend = blend;
            self.apply_blend();
        }
    }

    /// 上限に達していたら先に GPU へ送る。図形を積む前に呼ぶ。
    pub fn reserve(&mut self) {
        if self.batch.quads() >= MAX_QUADS {
            self.flush();
        }
    }

    pub fn flush(&mut self) {
        if self.batch.vertices.is_empty() {
            return;
        }
        let quads = self.batch.quads() as i32;
        unsafe {
            self.gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(&self.batch.vertices), glow::STREAM_DRAW);
            self.gl.draw_elements(glow::TRIANGLES, quads * 6, glow::UNSIGNED_SHORT, 0);
        }
        self.batch.vertices.clear();
    }

    pub fn end(&mut self) {
        self.flush();
    }

    /// 開発用: 現在のフレームバッファーを PPM で書き出す。
    #[cfg(debug_assertions)]
    pub fn dump_ppm(&self, width: u32, height: u32, path: &str) -> std::io::Result<()> {
        let mut px = vec![0u8; (width * height * 4) as usize];
        unsafe {
            self.gl.read_pixels(
                0, 0, width as i32, height as i32, glow::RGBA, glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut px)),
            );
        }
        let mut out = format!("P6 {width} {height} 255\n").into_bytes();
        for y in (0..height).rev() {
            for x in 0..width {
                let i = ((y * width + x) * 4) as usize;
                out.extend_from_slice(&px[i..i + 3]);
            }
        }
        std::fs::write(path, out)
    }
}

fn as_bytes<T: Copy>(s: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(s.as_ptr().cast(), std::mem::size_of_val(s)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_is_28_bytes() {
        assert_eq!(std::mem::size_of::<Vertex>(), 28);
    }

    #[test]
    fn line_quad_covers_capsule() {
        let mut b = Batch::default();
        b.line(10.0, 10.0, 20.0, 10.0, 2.0);
        assert_eq!(b.quads(), 1);
        let v = &b.vertices;
        // 半幅 1 + AA 2px 分だけ外側に広がる。
        assert_eq!(v[0].pos, [8.0, 8.0]);
        assert_eq!(v[3].pos, [22.0, 12.0]);
        assert_eq!(v[0].shape, [10.0, 1.0]);
        assert_eq!(v[1].local, [12.0, -2.0]);
    }

    #[test]
    fn degenerate_line_becomes_circle() {
        let mut b = Batch::default();
        b.line(5.0, 5.0, 5.0, 5.0, 4.0);
        assert_eq!(b.vertices[0].shape, [0.0, 2.0]);
    }

    #[test]
    fn color_clamps() {
        assert_eq!(color_u8(-1.0, 0.5, 2.0, 1.0), [0, 128, 255, 255]);
    }
}

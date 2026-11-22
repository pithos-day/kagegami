//! GLES3 のコンテキストでの 2D 描画（シェーダーは GLSL ES 1.00）。
//!
//! 図形は SDF として、1 図形 = 1 四角形で描く。ほとんどは「カプセル（線分を太らせた形）」で、
//! 円は長さ 0 のカプセル、線は丸い端のカプセル、矩形は判定を常に内側にした四角形。
//! 回転した矩形（`rect` に angle を渡したとき）だけは箱の SDF で、縁にアンチエイリアスを付ける。
//! 頂点はまとめて 1 回の draw call で送る。
//!
//! 画像（`image`・`subimage`）は別の頂点の形と別のシェーダーで描く（図形の頂点を大きくしないため）。
//! 画像のテクスチャーは色にアルファを掛けてあるので、ブレンドの係数も図形とは違う。

use glow::HasContext;

use crate::egl::EglContext;
use crate::gl_program::{self, ProgramError};
use crate::script::Kind;

/// 1 回の draw call に詰める四角形の上限（u16 インデックスに収まる数）。
pub const MAX_QUADS: usize = 8192;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    /// 画面上の位置（ピクセル、左上原点）。
    pub pos: [f32; 2],
    /// 図形のローカル座標（ピクセル）。カプセルは x が線分方向、y が法線方向で、線分の始点が原点。
    /// 回転した矩形は、矩形の中心が原点で、回す前の矩形の x・y 方向。
    pub local: [f32; 2],
    /// カプセルは (線分の長さ, 半径)。回転した矩形は (-半分の幅, 半分の高さ)。
    /// x が負なら回転した矩形（カプセルの長さは負にならない）。
    pub shape: [f32; 2],
    pub color: [u8; 4],
}

/// 画像の頂点。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImageVertex {
    /// 画面上の位置（ピクセル、左上原点）。
    pub pos: [f32; 2],
    /// テクスチャー座標。(0, 0) が画像の左上、(1, 1) が右下。
    pub uv: [f32; 2],
    /// 回転した画像の、中心を原点としたローカル座標（ピクセル）。回していなければ 0。
    pub local: [f32; 2],
    /// 回転した画像の半分の幅・高さ。0 なら回していない画像で、縁にアンチエイリアスを付けない。
    pub half: [f32; 2],
    pub color: [u8; 4],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    #[default]
    Alpha,
    Add,
}

/// GPU に依存しない頂点の組み立て部分（テストしやすいように分けている）。
#[derive(Default)]
pub struct Batch {
    pub vertices: Vec<Vertex>,
    pub color: [u8; 4],
}

/// 矩形を内側扱いにするための大きな半径（mediump の範囲に収まる値）。prelude.lua と揃える。
#[cfg(test)]
const FILL_RADIUS: f32 = 8192.0;
/// アンチエイリアスのために図形の外側へ広げる量（ピクセル）。prelude.lua と揃える。
#[cfg(test)]
const AA_PAD: f32 = 1.0;
/// 回転した矩形の半分の幅・高さがこれより小さければ描かない。prelude.lua と揃える。
/// shape.x を負にして回転した矩形の目印にしているので、0 に近い値（mediump で 0 に潰れる値）を避ける。
#[cfg(test)]
const MIN_HALF_SIZE: f32 = 1e-4;

impl Batch {
    pub fn quads(&self) -> usize {
        self.vertices.len() / 4
    }
}

/// 図形の頂点の参照実装。本番では prelude.lua が同じ計算をする（テストで両者を比べる）。
#[cfg(test)]
impl Batch {
    fn quad(&mut self, corners: [[f32; 2]; 4], locals: [[f32; 2]; 4], shape: [f32; 2]) {
        let color = self.color;
        for i in 0..4 {
            self.vertices.push(Vertex {
                pos: corners[i],
                local: locals[i],
                shape,
                color,
            });
        }
    }

    pub fn circle(&mut self, x: f32, y: f32, r: f32) {
        let e = r.max(0.0) + AA_PAD;
        self.quad(
            [
                [x - e, y - e],
                [x + e, y - e],
                [x - e, y + e],
                [x + e, y + e],
            ],
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
            [
                at(x1, y1, -e, -e),
                at(x2, y2, e, -e),
                at(x1, y1, -e, e),
                at(x2, y2, e, e),
            ],
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

    /// (x, y, w, h) の矩形を、その中心のまわりに angle ラジアン回す（y が下向きの画面で時計回り）。
    pub fn rotated_rect(&mut self, x: f32, y: f32, w: f32, h: f32, angle: f32) {
        let (hx, hy) = (w.abs() * 0.5, h.abs() * 0.5);
        if !(hx >= MIN_HALF_SIZE && hy >= MIN_HALF_SIZE) {
            return;
        }
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        let (s, c) = angle.sin_cos();
        let (ex, ey) = (hx + AA_PAD, hy + AA_PAD);
        let at = |lx: f32, ly: f32| [cx + lx * c - ly * s, cy + lx * s + ly * c];
        self.quad(
            [at(-ex, -ey), at(ex, -ey), at(-ex, ey), at(ex, ey)],
            [[-ex, -ey], [ex, -ey], [-ex, ey], [ex, ey]],
            [-hx, hy],
        );
    }
}

#[cfg(test)]
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
    float d;
    if (v_shape.x < 0.0) {
        // 回転した矩形: 中心が原点、半分の大きさは (-v_shape.x, v_shape.y) の箱の SDF。
        vec2 q = abs(v_local) - vec2(-v_shape.x, v_shape.y);
        d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
    } else {
        float dx = v_local.x - clamp(v_local.x, 0.0, v_shape.x);
        d = length(vec2(dx, v_local.y)) - v_shape.y;
    }
    float a = clamp(0.5 - d, 0.0, 1.0);
    gl_FragColor = vec4(v_color.rgb, v_color.a * a);
}
"#;

const IMAGE_VERT: &str = r#"
attribute vec2 a_pos;
attribute vec2 a_uv;
attribute vec2 a_local;
attribute vec2 a_half;
attribute vec4 a_color;
uniform vec2 u_res;
varying vec2 v_uv;
varying vec2 v_local;
varying vec2 v_half;
varying vec4 v_color;
void main() {
    v_uv = a_uv;
    v_local = a_local;
    v_half = a_half;
    v_color = a_color;
    vec2 ndc = a_pos / u_res * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
}
"#;

const IMAGE_FRAG: &str = r#"
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
uniform sampler2D u_tex;
varying vec2 v_uv;
varying vec2 v_local;
varying vec2 v_half;
varying vec4 v_color;
void main() {
    // ミップマップの選択に微分を使うので、分岐の外で読む。
    vec4 c = texture2D(u_tex, v_uv);
    float a = 1.0;
    if (v_half.x > 0.0) {
        // 回転した画像: 縁を箱の SDF でぼかす（回転した矩形と同じ）。
        vec2 q = abs(v_local) - v_half;
        float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
        a = clamp(0.5 - d, 0.0, 1.0);
    }
    // テクスチャーは色にアルファを掛けてあるので、色合いも掛けた形にして掛ける。
    gl_FragColor = c * vec4(v_color.rgb * v_color.a, v_color.a) * a;
}
"#;

/// 描く頂点の形。ブレンドの係数も、テクスチャーの色にアルファを掛けてあるかで変わる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Shapes,
    Images,
}

struct Pass {
    program: glow::Program,
    u_res: glow::UniformLocation,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
}

pub struct Renderer {
    gl: glow::Context,
    shapes: Pass,
    images: Pass,
    /// いま設定してある頂点の形とブレンド。
    layout: Layout,
    blend: Blend,
    texture: Option<glow::Texture>,
}

/// 頂点の属性（場所の番号, 要素数, 型, 正規化するか, 先頭からのバイト数）。
type Attrib = (&'static str, i32, u32, bool, i32);

const SHAPE_ATTRIBS: &[Attrib] = &[
    ("a_pos", 2, glow::FLOAT, false, 0),
    ("a_local", 2, glow::FLOAT, false, 8),
    ("a_shape", 2, glow::FLOAT, false, 16),
    ("a_color", 4, glow::UNSIGNED_BYTE, true, 24),
];

const IMAGE_ATTRIBS: &[Attrib] = &[
    ("a_pos", 2, glow::FLOAT, false, 0),
    ("a_uv", 2, glow::FLOAT, false, 8),
    ("a_local", 2, glow::FLOAT, false, 16),
    ("a_half", 2, glow::FLOAT, false, 24),
    ("a_color", 4, glow::UNSIGNED_BYTE, true, 32),
];

/// プログラムと、頂点の形を覚えた VAO を作る。`ibo` は VAO ごとに結び付ける。
unsafe fn pass(
    gl: &glow::Context,
    vert: &str,
    frag: &str,
    attribs: &[Attrib],
    stride: usize,
    ibo: glow::Buffer,
) -> Result<Pass, String> {
    unsafe {
        let program = gl_program::build(
            gl,
            &[(glow::VERTEX_SHADER, vert), (glow::FRAGMENT_SHADER, frag)],
            |program| {
                for (i, a) in attribs.iter().enumerate() {
                    gl.bind_attrib_location(program, i as u32, a.0);
                }
            },
        )
        .map_err(|e| match e {
            ProgramError::Create(e) => e,
            ProgramError::Compile { log, .. } => format!("shader compile: {log}"),
            ProgramError::Link(log) => format!("shader link: {log}"),
        })?;
        let u_res = gl
            .get_uniform_location(program, "u_res")
            .ok_or("u_res not found")?;
        let vao = gl.create_vertex_array()?;
        gl.bind_vertex_array(Some(vao));
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
        let vbo = gl.create_buffer()?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        for (i, &(_, size, ty, normalized, offset)) in attribs.iter().enumerate() {
            gl.enable_vertex_attrib_array(i as u32);
            gl.vertex_attrib_pointer_f32(i as u32, size, ty, normalized, stride as i32, offset);
        }
        gl.bind_vertex_array(None);
        Ok(Pass {
            program,
            u_res,
            vao,
            vbo,
        })
    }
}

impl Renderer {
    pub fn new(egl: &EglContext) -> Result<Self, String> {
        let gl = unsafe { glow::Context::from_loader_function(|s| egl.proc_address(s)) };
        unsafe {
            // 四角形 1 つ = 頂点 4 つ = 三角形 2 つ。インデックスは固定なので最初に 1 回だけ送る。
            let mut indices = Vec::with_capacity(MAX_QUADS * 6);
            for q in 0..MAX_QUADS as u16 {
                let b = q * 4;
                indices.extend_from_slice(&[b, b + 1, b + 2, b + 2, b + 1, b + 3]);
            }
            let ibo = gl.create_buffer()?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                as_bytes(&indices),
                glow::STATIC_DRAW,
            );
            let shapes = pass(
                &gl,
                VERT,
                FRAG,
                SHAPE_ATTRIBS,
                std::mem::size_of::<Vertex>(),
                ibo,
            )?;
            let images = pass(
                &gl,
                IMAGE_VERT,
                IMAGE_FRAG,
                IMAGE_ATTRIBS,
                std::mem::size_of::<ImageVertex>(),
                ibo,
            )?;
            // u_tex はテクスチャーユニット 0（uniform の既定値）。
            gl.enable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);

            Ok(Self {
                gl,
                shapes,
                images,
                layout: Layout::Shapes,
                blend: Blend::Alpha,
                texture: None,
            })
        }
    }

    fn apply_blend(&self) {
        unsafe {
            // 画像のテクスチャーは色にアルファを掛けてあるので、色にはアルファを掛けずに足す。
            let src = match self.layout {
                Layout::Shapes => glow::SRC_ALPHA,
                Layout::Images => glow::ONE,
            };
            let dst = match self.blend {
                Blend::Alpha => glow::ONE_MINUS_SRC_ALPHA,
                Blend::Add => glow::ONE,
            };
            // フレームバッファーのアルファは 1 のまま保つ。アルファが減ると、コンポジターが
            // その部分を半透明として扱い、背面が透けてしまう。
            self.gl.blend_func_separate(src, dst, glow::ZERO, glow::ONE);
        }
    }

    fn pass(&self, layout: Layout) -> &Pass {
        match layout {
            Layout::Shapes => &self.shapes,
            Layout::Images => &self.images,
        }
    }

    /// 頂点の形とブレンドを切り替える。変わっていなければ何もしない。
    fn set_state(&mut self, layout: Layout, blend: Blend) {
        if layout != self.layout {
            self.layout = layout;
            let p = self.pass(layout);
            unsafe {
                self.gl.use_program(Some(p.program));
                self.gl.bind_vertex_array(Some(p.vao));
                self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(p.vbo));
            }
        } else if blend == self.blend {
            return;
        }
        self.blend = blend;
        self.apply_blend();
    }

    pub fn gl(&self) -> &glow::Context {
        &self.gl
    }

    /// 1 フレームの描画を始める。GL の状態は出力間で共有されるので毎回設定し直す。
    /// `buffer` は描画するバッファーのピクセルサイズ、`screen` はスクリプトの座標系のサイズ。
    /// 両者が違うときは、座標が `screen` から `buffer` へ縮尺される。
    pub fn begin(&mut self, buffer: (u32, u32), screen: (u32, u32)) {
        unsafe {
            self.gl.viewport(0, 0, buffer.0 as i32, buffer.1 as i32);
            for p in [&self.images, &self.shapes] {
                self.gl.use_program(Some(p.program));
                self.gl
                    .uniform_2_f32(Some(&p.u_res), screen.0 as f32, screen.1 as f32);
            }
            self.gl.bind_vertex_array(Some(self.shapes.vao));
            self.gl
                .bind_buffer(glow::ARRAY_BUFFER, Some(self.shapes.vbo));
            self.gl.active_texture(glow::TEXTURE0);
        }
        self.layout = Layout::Shapes;
        self.blend = Blend::Alpha;
        self.texture = None;
        self.apply_blend();
    }

    pub fn clear(&mut self, r: f32, g: f32, b: f32) {
        unsafe {
            self.gl.clear_color(r, g, b, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    /// スクリプトが積んだ描画命令を描く。画像をまだテクスチャーにしていなければ、ここで作る。
    /// テクスチャーを作れなかったときはエラーを返す（何も描かない）。
    pub fn submit(
        &mut self,
        dl: &crate::script::DrawList,
        images: &crate::lua_images::Images,
    ) -> Result<(), String> {
        if dl.segments.iter().any(|s| s.kind != Kind::Shapes) {
            images.upload(&self.gl)?;
        }
        let [r, g, b] = dl.clear.unwrap_or([0.0; 3]);
        self.clear(r, g, b);
        for s in &dl.segments {
            match s.kind {
                Kind::Shapes => {
                    self.set_state(Layout::Shapes, s.blend);
                    self.draw_quads(&dl.batch.vertices[s.start * 4..(s.start + s.quads) * 4]);
                }
                Kind::Image(i) => {
                    let tex = images
                        .texture(i as usize)
                        .ok_or("internal: bad image index")?;
                    self.set_state(Layout::Images, s.blend);
                    if self.texture != Some(tex) {
                        self.texture = Some(tex);
                        unsafe { self.gl.bind_texture(glow::TEXTURE_2D, Some(tex)) };
                    }
                    self.draw_quads(&dl.images[s.start * 4..(s.start + s.quads) * 4]);
                }
            }
        }
        Ok(())
    }

    /// 四角形を、MAX_QUADS 個ずつに分けて描く。
    fn draw_quads<T: Copy>(&self, vertices: &[T]) {
        for chunk in vertices.chunks(MAX_QUADS * 4) {
            unsafe {
                self.gl.buffer_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    as_bytes(chunk),
                    glow::STREAM_DRAW,
                );
                self.gl.draw_elements(
                    glow::TRIANGLES,
                    (chunk.len() / 4 * 6) as i32,
                    glow::UNSIGNED_SHORT,
                    0,
                );
            }
        }
    }

    /// 開発用: 現在のフレームバッファーを PPM で書き出す。
    #[cfg(debug_assertions)]
    pub fn dump_ppm(&self, width: u32, height: u32, path: &str) -> std::io::Result<()> {
        let mut px = vec![0u8; (width * height * 4) as usize];
        unsafe {
            self.gl.read_pixels(
                0,
                0,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
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
        std::fs::write(path, out)?;
        // アルファの確認用に、不透明でない画素の数を書き出す。
        let translucent = px.as_chunks::<4>().0.iter().filter(|p| p[3] != 255).count();
        std::fs::write(format!("{path}.alpha"), format!("{translucent}\n"))
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
    fn image_vertex_is_36_bytes() {
        assert_eq!(std::mem::size_of::<ImageVertex>(), 36);
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
    fn rotated_rect_quad_covers_box() {
        let mut b = Batch::default();
        b.rotated_rect(10.0, 20.0, 40.0, 20.0, 0.0);
        assert_eq!(b.quads(), 1);
        let v = &b.vertices;
        // 中心 (30, 30)、半分の大きさ (20, 10) に、AA の 1px を足して広げる。
        assert_eq!(v[0].pos, [9.0, 19.0]);
        assert_eq!(v[3].pos, [51.0, 41.0]);
        assert_eq!(v[0].local, [-21.0, -11.0]);
        assert_eq!(v[0].shape, [-20.0, 10.0]);
    }

    #[test]
    fn rotated_rect_turns_clockwise_on_screen_around_its_center() {
        let mut b = Batch::default();
        b.rotated_rect(10.0, 20.0, 40.0, 20.0, std::f32::consts::FRAC_PI_2);
        // 左上の角（ローカル (-21, -11)）は、y が下向きの画面で時計回りに 90 度回ると右上へ行く。
        let p = b.vertices[0].pos;
        assert!(
            (p[0] - 41.0).abs() < 1e-4 && (p[1] - 9.0).abs() < 1e-4,
            "{p:?}"
        );
    }

    #[test]
    fn rotated_rect_with_negative_size_uses_the_same_box() {
        let mut a = Batch::default();
        a.rotated_rect(10.0, 20.0, 40.0, 20.0, 0.3);
        let mut b = Batch::default();
        b.rotated_rect(50.0, 40.0, -40.0, -20.0, 0.3);
        assert_eq!(a.vertices, b.vertices);
    }

    #[test]
    fn rotated_rect_with_empty_or_nan_size_draws_nothing() {
        let mut b = Batch::default();
        b.rotated_rect(0.0, 0.0, 0.0, 5.0, 1.0);
        b.rotated_rect(0.0, 0.0, 5.0, f32::NAN, 1.0);
        assert_eq!(b.quads(), 0);
    }

    #[test]
    fn color_clamps() {
        assert_eq!(color_u8(-1.0, 0.5, 2.0, 1.0), [0, 128, 255, 255]);
    }
}

//! シェーダーの壁紙のバッファー（Buffer A〜D、#57）の GPU 側。
//!
//! - 形式は RGBA32F（Shadertoy の 2D のバッファーと同じ）。描き込むには `GL_EXT_color_buffer_float` が要る
//! - バッファーごとにテクスチャーを 2 枚持ち、前のフレームの結果を読みながら、もう 1 枚へ描く
//! - 出力ごとに持つ（サイズは描画するバッファーのピクセル）。サイズが変わったら、重なる範囲を左下から写す
//! - バッファーを読むときの filter・wrap はチャンネルごとに違うので、sampler オブジェクトで決める。
//!   `GL_OES_texture_float_linear` がなければ、linear・mipmap を nearest にする（Shadertoy と同じ）

use glow::HasContext;

use crate::directive::{BUFFER_NAMES, Filter, Sampler, Wrap};

/// バッファーを使えるか確かめる。使えれば、線形フィルターで読めるか（`GL_OES_texture_float_linear`）を返す。
pub fn check_support(gl: &glow::Context) -> Result<bool, String> {
    let ext = gl.supported_extensions();
    if !ext.contains("GL_EXT_color_buffer_float") {
        return Err(
            "buffers (@bufferA to @bufferD) need GL_EXT_color_buffer_float \
                    to render to RGBA32F textures, which is not available"
                .into(),
        );
    }
    Ok(ext.contains("GL_OES_texture_float_linear"))
}

/// 1 つのバッファー。
struct Buffer {
    tex: [glow::Texture; 2],
    fbo: [glow::Framebuffer; 2],
    /// 最後に描き終えた方（読む方）。
    current: usize,
    /// ミップマップを作るか（どれかのパスが mipmap で読み、線形フィルターで読める）。
    mipmap: bool,
}

/// 1 つの出力のバッファーのすべて。
pub struct BufferSet {
    size: (u32, u32),
    bufs: [Option<Buffer>; 4],
    /// filter（nearest・linear・mipmap）× wrap（clamp・repeat）の sampler。
    samplers: [glow::Sampler; 6],
}

/// ミップマップのすべての段の数。
fn mip_levels(size: (u32, u32)) -> i32 {
    (32 - size.0.max(size.1).max(1).leading_zeros()) as i32
}

/// 前に溜まったエラーを消す（下で、作ったものだけのエラーを見るため）。
/// コンテキストを失ったときにエラーが出続けても止まらないように、回数を限る。
fn drain_errors(gl: &glow::Context) {
    for _ in 0..16 {
        if unsafe { gl.get_error() } == glow::NO_ERROR {
            break;
        }
    }
}

impl Buffer {
    /// 0 で塗った 2 枚のテクスチャーを作る。
    fn new(gl: &glow::Context, name: &str, size: (u32, u32), mipmap: bool) -> Result<Self, String> {
        let levels = if mipmap { mip_levels(size) } else { 1 };
        let mut tex = Vec::with_capacity(2);
        let mut fbo = Vec::with_capacity(2);
        let result = (|| unsafe {
            for _ in 0..2 {
                let t = gl.create_texture()?;
                tex.push(t);
                gl.bind_texture(glow::TEXTURE_2D, Some(t));
                gl.tex_storage_2d(
                    glow::TEXTURE_2D,
                    levels,
                    glow::RGBA32F,
                    size.0 as i32,
                    size.1 as i32,
                );
                // sampler で上書きするが、sampler を使わずに読んだときにも完全なテクスチャーにしておく。
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::NEAREST as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::NEAREST as i32,
                );
                let f = gl.create_framebuffer()?;
                fbo.push(f);
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(t),
                    0,
                );
                let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
                if status != glow::FRAMEBUFFER_COMPLETE {
                    return Err(format!(
                        "cannot create {name} ({}x{} RGBA32F): framebuffer is incomplete (0x{status:X})",
                        size.0, size.1
                    ));
                }
                gl.clear_color(0.0, 0.0, 0.0, 0.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                // tex_storage_2d の中身は不定なので、ミップマップの段も 0 にしておく
                // （最初のフレームで自分を mipmap で読んだときに、不定な値が残り続けないように）。
                if levels > 1 {
                    gl.generate_mipmap(glow::TEXTURE_2D);
                }
            }
            // VRAM が足りないときなど。
            match gl.get_error() {
                glow::NO_ERROR => Ok(()),
                e => Err(format!(
                    "cannot create {name} ({}x{} RGBA32F, GL error 0x{e:X})",
                    size.0, size.1
                )),
            }
        })();
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        match result {
            Ok(()) => Ok(Self {
                tex: [tex[0], tex[1]],
                fbo: [fbo[0], fbo[1]],
                current: 0,
                mipmap,
            }),
            Err(e) => {
                unsafe {
                    for t in tex {
                        gl.delete_texture(t);
                    }
                    for f in fbo {
                        gl.delete_framebuffer(f);
                    }
                }
                Err(e)
            }
        }
    }

    fn generate_mipmap(&self, gl: &glow::Context, i: usize) {
        if self.mipmap {
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.tex[i]));
                gl.generate_mipmap(glow::TEXTURE_2D);
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
        }
    }

    fn delete(self, gl: &glow::Context) {
        unsafe {
            for t in self.tex {
                gl.delete_texture(t);
            }
            for f in self.fbo {
                gl.delete_framebuffer(f);
            }
        }
    }
}

const FILTERS: [Filter; 3] = [Filter::Nearest, Filter::Linear, Filter::Mipmap];
const WRAPS: [Wrap; 2] = [Wrap::Clamp, Wrap::Repeat];

fn sampler_index(s: Sampler) -> usize {
    let f = FILTERS.iter().position(|f| *f == s.filter).unwrap_or(0);
    let w = WRAPS.iter().position(|w| *w == s.wrap).unwrap_or(0);
    f * WRAPS.len() + w
}

impl BufferSet {
    /// `declared` のバッファーを作る。`mipmap` はどれかのパスが mipmap で読むバッファー。
    /// `float_linear` は `check_support` の結果。
    pub fn new(
        gl: &glow::Context,
        size: (u32, u32),
        declared: [bool; 4],
        mipmap: [bool; 4],
        float_linear: bool,
    ) -> Result<Self, String> {
        drain_errors(gl);
        let mut samplers = Vec::with_capacity(6);
        for filter in FILTERS {
            for wrap in WRAPS {
                let s = unsafe { gl.create_sampler()? };
                // 線形フィルターで読めなければ、Shadertoy と同じく nearest にする。
                let filter = if float_linear {
                    filter
                } else {
                    Filter::Nearest
                };
                let (min, mag) = match filter {
                    Filter::Nearest => (glow::NEAREST, glow::NEAREST),
                    Filter::Linear => (glow::LINEAR, glow::LINEAR),
                    Filter::Mipmap => (glow::LINEAR_MIPMAP_LINEAR, glow::LINEAR),
                };
                let wrap = match wrap {
                    Wrap::Clamp => glow::CLAMP_TO_EDGE,
                    Wrap::Repeat => glow::REPEAT,
                } as i32;
                unsafe {
                    gl.sampler_parameter_i32(s, glow::TEXTURE_MIN_FILTER, min as i32);
                    gl.sampler_parameter_i32(s, glow::TEXTURE_MAG_FILTER, mag as i32);
                    gl.sampler_parameter_i32(s, glow::TEXTURE_WRAP_S, wrap);
                    gl.sampler_parameter_i32(s, glow::TEXTURE_WRAP_T, wrap);
                }
                samplers.push(s);
            }
        }
        let mut set = Self {
            size,
            bufs: Default::default(),
            samplers: samplers.try_into().unwrap_or_else(|_| unreachable!()),
        };
        for i in 0..4 {
            if !declared[i] {
                continue;
            }
            match Buffer::new(gl, BUFFER_NAMES[i], size, mipmap[i] && float_linear) {
                Ok(b) => set.bufs[i] = Some(b),
                Err(e) => {
                    set.delete(gl);
                    return Err(e);
                }
            }
        }
        Ok(set)
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// サイズを変える。重なる範囲（左下から）の中身を写し、ほかは 0 にする（Shadertoy と同じ）。
    /// 作れなければ、今のバッファーを残してエラーを返す。
    pub fn resize(&mut self, gl: &glow::Context, size: (u32, u32)) -> Result<(), String> {
        if size == self.size {
            return Ok(());
        }
        drain_errors(gl);
        let mut new: [Option<Buffer>; 4] = Default::default();
        for i in 0..4 {
            let Some(old) = &self.bufs[i] else { continue };
            match Buffer::new(gl, BUFFER_NAMES[i], size, old.mipmap) {
                Ok(b) => new[i] = Some(b),
                Err(e) => {
                    for b in new.into_iter().flatten() {
                        b.delete(gl);
                    }
                    return Err(e);
                }
            }
        }
        let (w, h) = (
            size.0.min(self.size.0) as i32,
            size.1.min(self.size.1) as i32,
        );
        for (old, new) in self.bufs.iter_mut().zip(new) {
            let (Some(o), Some(mut n)) = (old.take(), new) else {
                continue;
            };
            n.current = o.current;
            for t in 0..2 {
                unsafe {
                    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(o.fbo[t]));
                    gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(n.fbo[t]));
                    gl.blit_framebuffer(
                        0,
                        0,
                        w,
                        h,
                        0,
                        0,
                        w,
                        h,
                        glow::COLOR_BUFFER_BIT,
                        glow::NEAREST,
                    );
                }
                n.generate_mipmap(gl, t);
            }
            o.delete(gl);
            *old = Some(n);
        }
        unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, None) };
        self.size = size;
        Ok(())
    }

    /// バッファー `i` の読む方のテクスチャー（前に描き終えた方）。
    pub fn texture(&self, i: usize) -> Option<glow::Texture> {
        self.bufs[i].as_ref().map(|b| b.tex[b.current])
    }

    pub fn sampler(&self, s: Sampler) -> glow::Sampler {
        self.samplers[sampler_index(s)]
    }

    /// バッファー `i` の、読まない方へ描き始める。
    pub fn begin(&self, gl: &glow::Context, i: usize) {
        if let Some(b) = &self.bufs[i] {
            unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, Some(b.fbo[1 - b.current])) };
        }
    }

    /// バッファー `i` を描き終える。描いた方を読む方にして、要ればミップマップを作る。
    pub fn end(&mut self, gl: &glow::Context, i: usize) {
        unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, None) };
        if let Some(b) = &mut self.bufs[i] {
            b.current = 1 - b.current;
            b.generate_mipmap(gl, b.current);
        }
    }

    pub fn delete(self, gl: &glow::Context) {
        for b in self.bufs.into_iter().flatten() {
            b.delete(gl);
        }
        for s in self.samplers {
            unsafe { gl.delete_sampler(s) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_levels_cover_down_to_one_pixel() {
        assert_eq!(mip_levels((1, 1)), 1);
        assert_eq!(mip_levels((2, 1)), 2);
        assert_eq!(mip_levels((1920, 1080)), 11);
        assert_eq!(mip_levels((3840, 2160)), 12);
        assert_eq!(mip_levels((4096, 16)), 13);
    }

    #[test]
    fn every_sampler_has_its_own_slot() {
        let mut seen = std::collections::HashSet::new();
        for filter in FILTERS {
            for wrap in WRAPS {
                let i = sampler_index(Sampler {
                    filter,
                    wrap,
                    vflip: false,
                });
                assert!(i < 6);
                assert!(seen.insert(i));
            }
        }
    }
}

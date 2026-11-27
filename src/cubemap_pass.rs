//! シェーダーの壁紙の Cubemap A（キューブマップに描くパス、#72）の GPU 側。
//!
//! - 形式は RGBA16F、面のサイズは 1024（Shadertoy と同じ）。描き込むには `GL_EXT_color_buffer_float` か
//!   `GL_EXT_color_buffer_half_float` が要る。RGBA16F は GLES 3.0 で線形フィルターで読める
//! - キューブマップを 2 枚持ち、前のフレームの結果を読みながら、もう 1 枚の 6 面へ描く
//! - 出力ごとに持つ（Cubemap A の中の `iResolution` が出力のサイズのため）。出力のサイズが変わっても
//!   面のサイズは変わらないので、作り直さずに中身を残す（Shadertoy と同じ）
//! - 読むときの filter はチャンネルごとに違うので、sampler オブジェクトで決める

use glow::HasContext;

use crate::directive::{CUBEMAP_NAME, Filter};

/// Cubemap A の面のサイズ（Shadertoy と同じ）。
pub const FACE_SIZE: u32 = 1024;

/// Cubemap A を使えるか確かめる。
pub fn check_support(gl: &glow::Context) -> Result<(), String> {
    let ext = gl.supported_extensions();
    if ext.contains("GL_EXT_color_buffer_float") || ext.contains("GL_EXT_color_buffer_half_float") {
        return Ok(());
    }
    Err(format!(
        "@{CUBEMAP_NAME} needs GL_EXT_color_buffer_float or GL_EXT_color_buffer_half_float \
         to render to RGBA16F textures, which is not available"
    ))
}

/// ミップマップのすべての段の数。
fn mip_levels(size: u32) -> i32 {
    (32 - size.max(1).leading_zeros()) as i32
}

/// Cubemap A のテクスチャー（1 つの出力のもの）。
pub struct CubemapPass {
    size: u32,
    tex: [glow::Texture; 2],
    /// `fbo[i][face]`: キューブマップ i の面 face（+X, -X, +Y, -Y, +Z, -Z の順）に描く。
    fbo: [[glow::Framebuffer; 6]; 2],
    /// 最後に描き終えた方（読む方）。
    current: usize,
    /// ミップマップを作るか（どれかのパスが mipmap で読む）。
    mipmap: bool,
    /// filter（nearest・linear・mipmap）ごとの sampler。
    samplers: [glow::Sampler; 3],
}

/// 前に溜まったエラーを消す（下で、作ったものだけのエラーを見るため）。
fn drain_errors(gl: &glow::Context) {
    for _ in 0..16 {
        if unsafe { gl.get_error() } == glow::NO_ERROR {
            break;
        }
    }
}

impl CubemapPass {
    /// 0 で塗った、面のサイズが `size` のキューブマップを 2 枚作る。アプリでは `FACE_SIZE` を渡す。
    pub fn new(gl: &glow::Context, size: u32, mipmap: bool) -> Result<Self, String> {
        drain_errors(gl);
        let levels = if mipmap { mip_levels(size) } else { 1 };
        let mut tex = Vec::with_capacity(2);
        let mut fbo = Vec::with_capacity(12);
        let mut samplers = Vec::with_capacity(3);
        let what = || format!("{CUBEMAP_NAME} ({size}x{size}x6 RGBA16F)");
        let result = (|| unsafe {
            for _ in 0..2 {
                let t = gl.create_texture()?;
                tex.push(t);
                gl.bind_texture(glow::TEXTURE_CUBE_MAP, Some(t));
                gl.tex_storage_2d(
                    glow::TEXTURE_CUBE_MAP,
                    levels,
                    glow::RGBA16F,
                    size as i32,
                    size as i32,
                );
                // sampler で上書きするが、sampler を使わずに読んだときにも完全なテクスチャーにしておく。
                for (p, v) in [
                    (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
                    (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_CUBE_MAP, p, v as i32);
                }
                for face in 0..6 {
                    let f = gl.create_framebuffer()?;
                    fbo.push(f);
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(f));
                    gl.framebuffer_texture_2d(
                        glow::FRAMEBUFFER,
                        glow::COLOR_ATTACHMENT0,
                        glow::TEXTURE_CUBE_MAP_POSITIVE_X + face,
                        Some(t),
                        0,
                    );
                    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
                    if status != glow::FRAMEBUFFER_COMPLETE {
                        return Err(format!(
                            "cannot create {}: framebuffer is incomplete (0x{status:X})",
                            what()
                        ));
                    }
                    gl.clear_color(0.0, 0.0, 0.0, 0.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                }
                // tex_storage_2d の中身は不定なので、ミップマップの段も 0 にしておく。
                if levels > 1 {
                    gl.generate_mipmap(glow::TEXTURE_CUBE_MAP);
                }
            }
            for filter in FILTERS {
                let s = gl.create_sampler()?;
                samplers.push(s);
                let (min, mag) = match filter {
                    Filter::Nearest => (glow::NEAREST, glow::NEAREST),
                    Filter::Linear => (glow::LINEAR, glow::LINEAR),
                    Filter::Mipmap => (glow::LINEAR_MIPMAP_LINEAR, glow::LINEAR),
                };
                gl.sampler_parameter_i32(s, glow::TEXTURE_MIN_FILTER, min as i32);
                gl.sampler_parameter_i32(s, glow::TEXTURE_MAG_FILTER, mag as i32);
                // 面の境目は常につながる（GLES 3.0）ので描画には影響しないが、Shadertoy の clamp に合わせる。
                for p in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
                    gl.sampler_parameter_i32(s, p, glow::CLAMP_TO_EDGE as i32);
                }
            }
            // VRAM が足りないときなど。
            match gl.get_error() {
                glow::NO_ERROR => Ok(()),
                e => Err(format!("cannot create {} (GL error 0x{e:X})", what())),
            }
        })();
        unsafe {
            gl.bind_texture(glow::TEXTURE_CUBE_MAP, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        if let Err(e) = result {
            unsafe {
                tex.into_iter().for_each(|t| gl.delete_texture(t));
                fbo.into_iter().for_each(|f| gl.delete_framebuffer(f));
                samplers.into_iter().for_each(|s| gl.delete_sampler(s));
            }
            return Err(e);
        }
        let fbo: [glow::Framebuffer; 12] = fbo.try_into().unwrap_or_else(|_| unreachable!());
        Ok(Self {
            size,
            tex: [tex[0], tex[1]],
            fbo: [
                fbo[..6].try_into().unwrap_or_else(|_| unreachable!()),
                fbo[6..].try_into().unwrap_or_else(|_| unreachable!()),
            ],
            current: 0,
            mipmap,
            samplers: samplers.try_into().unwrap_or_else(|_| unreachable!()),
        })
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// 読む方のキューブマップ（前に描き終えた方）。
    pub fn texture(&self) -> glow::Texture {
        self.tex[self.current]
    }

    pub fn sampler(&self, filter: Filter) -> glow::Sampler {
        self.samplers[FILTERS.iter().position(|f| *f == filter).unwrap_or(0)]
    }

    /// 読まない方のキューブマップの、面 `face`（0〜5、+X, -X, +Y, -Y, +Z, -Z）へ描き始める。
    pub fn begin_face(&self, gl: &glow::Context, face: usize) {
        unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo[1 - self.current][face])) };
    }

    /// 6 面を描き終える。描いた方を読む方にして、要ればミップマップを作る。
    pub fn end(&mut self, gl: &glow::Context) {
        unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, None) };
        self.current = 1 - self.current;
        if self.mipmap {
            unsafe {
                gl.bind_texture(glow::TEXTURE_CUBE_MAP, Some(self.tex[self.current]));
                gl.generate_mipmap(glow::TEXTURE_CUBE_MAP);
                gl.bind_texture(glow::TEXTURE_CUBE_MAP, None);
            }
        }
    }

    pub fn delete(self, gl: &glow::Context) {
        unsafe {
            self.tex.into_iter().for_each(|t| gl.delete_texture(t));
            self.fbo
                .into_iter()
                .flatten()
                .for_each(|f| gl.delete_framebuffer(f));
            self.samplers.into_iter().for_each(|s| gl.delete_sampler(s));
        }
    }
}

const FILTERS: [Filter; 3] = [Filter::Nearest, Filter::Linear, Filter::Mipmap];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_levels_cover_down_to_one_pixel() {
        assert_eq!(mip_levels(1), 1);
        assert_eq!(mip_levels(16), 5);
        assert_eq!(mip_levels(FACE_SIZE), 11);
    }
}

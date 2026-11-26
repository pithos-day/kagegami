//! 画像を GL のテクスチャーにする（#36）。
//!
//! EGL のコンテキストはすべての出力で 1 つなので、テクスチャーは出力の間で共有できる。

use glow::HasContext;

use crate::directive::{Filter, Sampler, Wrap};
use crate::image::Image;
use crate::noise::{self, Noise};

pub struct Texture {
    pub tex: glow::Texture,
    pub width: u32,
    pub height: u32,
    /// 3D テクスチャーの奥行き。2D は 1。
    pub depth: u32,
}

impl Texture {
    /// `bind_texture` に渡すターゲット。
    pub fn target(&self) -> u32 {
        if self.depth > 1 {
            glow::TEXTURE_3D
        } else {
            glow::TEXTURE_2D
        }
    }
}

/// このコンテキストで作れるテクスチャーの縦横の上限。
pub fn max_size(gl: &glow::Context) -> u32 {
    unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) }.max(1) as u32
}

/// 前に溜まったエラーを消す（テクスチャーを作った後に、そのテクスチャーのエラーだけを見るため）。
/// コンテキストを失ったときにエラーが出続けても止まらないように、回数を限る。
fn drain_errors(gl: &glow::Context) {
    for _ in 0..16 {
        if unsafe { gl.get_error() } == glow::NO_ERROR {
            break;
        }
    }
}

/// `target` にバインドしたテクスチャーに、filter と wrap を設定する。mipmap ならミップマップも作る。
unsafe fn set_sampler(gl: &glow::Context, target: u32, sampler: Sampler) {
    let (min, mag) = match sampler.filter {
        Filter::Nearest => (glow::NEAREST, glow::NEAREST),
        Filter::Linear => (glow::LINEAR, glow::LINEAR),
        Filter::Mipmap => (glow::LINEAR_MIPMAP_LINEAR, glow::LINEAR),
    };
    let wrap = match sampler.wrap {
        Wrap::Clamp => glow::CLAMP_TO_EDGE,
        Wrap::Repeat => glow::REPEAT,
    } as i32;
    unsafe {
        if sampler.filter == Filter::Mipmap {
            gl.generate_mipmap(target);
        }
        gl.tex_parameter_i32(target, glow::TEXTURE_MIN_FILTER, min as i32);
        gl.tex_parameter_i32(target, glow::TEXTURE_MAG_FILTER, mag as i32);
        gl.tex_parameter_i32(target, glow::TEXTURE_WRAP_S, wrap);
        gl.tex_parameter_i32(target, glow::TEXTURE_WRAP_T, wrap);
        if target == glow::TEXTURE_3D {
            gl.tex_parameter_i32(target, glow::TEXTURE_WRAP_R, wrap);
        }
    }
}

/// 作ったテクスチャーのエラーを確かめる。エラーならテクスチャーを消す。
fn finish(gl: &glow::Context, t: Texture) -> Result<Texture, String> {
    // 画像が大きくて GPU のメモリーが足りないときなど。
    match unsafe { gl.get_error() } {
        glow::NO_ERROR => Ok(t),
        e => {
            unsafe { gl.delete_texture(t.tex) };
            Err(format!("cannot create a texture (GL error 0x{e:X})"))
        }
    }
}

/// RGBA8 のテクスチャーを作る。`img` の先頭の行がテクスチャーの t = 0 になる。
pub fn upload(gl: &glow::Context, img: &Image, sampler: Sampler) -> Result<Texture, String> {
    drain_errors(gl);
    unsafe {
        let tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            img.width as i32,
            img.height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&img.rgba)),
        );
        set_sampler(gl, glow::TEXTURE_2D, sampler);
        gl.bind_texture(glow::TEXTURE_2D, None);
        finish(
            gl,
            Texture {
                tex,
                width: img.width,
                height: img.height,
                depth: 1,
            },
        )
    }
}

/// 組み込みの 3D ノイズのテクスチャーを作る（#70）。グレーは R8、RGBA は RGBA8。
pub fn upload_noise(gl: &glow::Context, noise: Noise, sampler: Sampler) -> Result<Texture, String> {
    let (internal, format) = match noise {
        Noise::Grey => (glow::R8, glow::RED),
        Noise::Rgba => (glow::RGBA8, glow::RGBA),
    };
    let n = noise::SIZE as i32;
    drain_errors(gl);
    unsafe {
        let tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_3D, Some(tex));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_3d(
            glow::TEXTURE_3D,
            0,
            internal as i32,
            n,
            n,
            n,
            0,
            format,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(noise.data())),
        );
        set_sampler(gl, glow::TEXTURE_3D, sampler);
        gl.bind_texture(glow::TEXTURE_3D, None);
        finish(
            gl,
            Texture {
                tex,
                width: noise::SIZE,
                height: noise::SIZE,
                depth: noise::SIZE,
            },
        )
    }
}

pub fn delete(gl: &glow::Context, t: Texture) {
    unsafe { gl.delete_texture(t.tex) };
}

/// 黒い 1×1 のテクスチャー（画像を渡していないチャンネル用）。
pub fn black(gl: &glow::Context) -> Result<Texture, String> {
    let img = Image {
        width: 1,
        height: 1,
        rgba: vec![0, 0, 0, 255],
    };
    upload(
        gl,
        &img,
        Sampler {
            filter: Filter::Nearest,
            wrap: Wrap::Clamp,
            vflip: false,
        },
    )
}

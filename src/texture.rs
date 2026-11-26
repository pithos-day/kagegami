//! 画像を GL のテクスチャーにする（#36）。
//!
//! EGL のコンテキストはすべての出力で 1 つなので、テクスチャーは出力の間で共有できる。

use glow::HasContext;

use crate::directive::{Filter, Sampler, Wrap};
use crate::image::Image;

pub struct Texture {
    pub tex: glow::Texture,
    pub width: u32,
    pub height: u32,
}

/// このコンテキストで作れるテクスチャーの縦横の上限。
pub fn max_size(gl: &glow::Context) -> u32 {
    unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) }.max(1) as u32
}

/// RGBA8 のテクスチャーを作る。`img` の先頭の行がテクスチャーの t = 0 になる。
pub fn upload(gl: &glow::Context, img: &Image, sampler: Sampler) -> Result<Texture, String> {
    unsafe {
        // 前に溜まったエラーを消してから作る（下で、このテクスチャーのエラーだけを見るため）。
        // コンテキストを失ったときにエラーが出続けても止まらないように、回数を限る。
        for _ in 0..16 {
            if gl.get_error() == glow::NO_ERROR {
                break;
            }
        }
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
        let (min, mag) = match sampler.filter {
            Filter::Nearest => (glow::NEAREST, glow::NEAREST),
            Filter::Linear => (glow::LINEAR, glow::LINEAR),
            Filter::Mipmap => (glow::LINEAR_MIPMAP_LINEAR, glow::LINEAR),
        };
        if sampler.filter == Filter::Mipmap {
            gl.generate_mipmap(glow::TEXTURE_2D);
        }
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, mag as i32);
        let wrap = match sampler.wrap {
            Wrap::Clamp => glow::CLAMP_TO_EDGE,
            Wrap::Repeat => glow::REPEAT,
        } as i32;
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, wrap);
        gl.bind_texture(glow::TEXTURE_2D, None);
        // 画像が大きくて GPU のメモリーが足りないときなど。
        match gl.get_error() {
            glow::NO_ERROR => Ok(Texture {
                tex,
                width: img.width,
                height: img.height,
            }),
            e => {
                gl.delete_texture(tex);
                Err(format!("cannot create a texture (GL error 0x{e:X})"))
            }
        }
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

/// キーボードの入力のテクスチャー（#69）。Shadertoy と同じ R8 の 256×3 で、nearest・clamp で読む。
pub fn keyboard(gl: &glow::Context) -> Result<Texture, String> {
    use crate::keyboard::{TEXTURE_HEIGHT, TEXTURE_WIDTH};
    unsafe {
        let tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::R8 as i32,
            TEXTURE_WIDTH as i32,
            TEXTURE_HEIGHT as i32,
            0,
            glow::RED,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&[0u8; TEXTURE_WIDTH * TEXTURE_HEIGHT])),
        );
        for (p, v) in [
            (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
            (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        Ok(Texture {
            tex,
            width: TEXTURE_WIDTH as u32,
            height: TEXTURE_HEIGHT as u32,
        })
    }
}

/// キーボードの入力のテクスチャーを、`keyboard::Keys::texture` の中身にする。
pub fn update_keyboard(gl: &glow::Context, t: &Texture, data: &[u8]) {
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(t.tex));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            0,
            0,
            t.width as i32,
            t.height as i32,
            glow::RED,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(data)),
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
}

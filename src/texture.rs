//! 画像を GL のテクスチャーにする（#36）。正距円筒図法の画像からキューブマップも作る（#68）。
//!
//! EGL のコンテキストはすべての出力で 1 つなので、テクスチャーは出力の間で共有できる。

use glow::HasContext;

use crate::directive::{Filter, Sampler, Wrap};
use crate::image::Image;
use crate::noise::{self, Noise};

pub struct Texture {
    pub tex: glow::Texture,
    /// `bind_texture` に渡すターゲット（`TEXTURE_2D`・`TEXTURE_3D`・`TEXTURE_CUBE_MAP`）。
    pub target: u32,
    /// 縦横。キューブマップは面のサイズ。
    pub width: u32,
    pub height: u32,
    /// 3D テクスチャーの奥行き。2D とキューブマップは 1。
    pub depth: u32,
}

/// このコンテキストで作れるテクスチャーの縦横の上限。
pub fn max_size(gl: &glow::Context) -> u32 {
    unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) }.max(1) as u32
}

/// このコンテキストで作れるキューブマップの面の縦横の上限。
pub fn max_cube_size(gl: &glow::Context) -> u32 {
    unsafe { gl.get_parameter_i32(glow::MAX_CUBE_MAP_TEXTURE_SIZE) }.max(1) as u32
}

/// 正距円筒図法の画像（`width`×`height`）から作るキューブマップの、面のサイズ（#68）。
/// 縦横比がちょうど 2:1 でない画像と、面が `max_cube` を超える画像はエラーにする。
/// 面のサイズは幅 ÷ 4（赤道付近の画素の細かさがほぼ変わらない）。割り切れなければ切り捨てる。
pub fn cubemap_face_size(width: u32, height: u32, max_cube: u32) -> Result<u32, String> {
    if width != height.saturating_mul(2) {
        return Err(format!(
            "a cubemap must be an equirectangular image with a 2:1 aspect ratio (got {width}x{height})"
        ));
    }
    let face = width / 4;
    if face == 0 {
        return Err(format!(
            "image is too small for a cubemap ({width}x{height}, min 4x2)"
        ));
    }
    if face > max_cube {
        return Err(format!(
            "cubemap faces are too large ({face}x{face} from {width}x{height}, \
             max {max_cube}x{max_cube} on this GPU)"
        ));
    }
    Ok(face)
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
        if target == glow::TEXTURE_3D || target == glow::TEXTURE_CUBE_MAP {
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
                target: glow::TEXTURE_2D,
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
                target: glow::TEXTURE_3D,
                width: noise::SIZE,
                height: noise::SIZE,
                depth: noise::SIZE,
            },
        )
    }
}

/// 全画面を覆う三角形 1 つ（shader.rs と同じ）。
const CUBE_VERT: &str = "#version 300 es
void main() {
    vec2 p = vec2(float((gl_VertexID & 1) << 2), float((gl_VertexID & 2) << 1)) - 1.0;
    gl_Position = vec4(p, 0.0, 1.0);
}
";

/// キューブマップの 1 つの面に、正距円筒図法の画像を写す（#68）。
///
/// 面の画素 (s, t) の方向は、GLES 3.0 の仕様のキューブマップの面の選び方（sc・tc・ma の表）の逆。
/// 方向 d の経度・緯度は、画像の中心を -Z、右へ 1/4 進んだ位置を +X、上を +Y として
/// d = (sin(lon)·cos(lat), sin(lat), -cos(lon)·cos(lat)) から求める。
/// 画像は上の行から順にテクスチャーにしてあるので、t = 0 が画像の上（緯度 +90°）。
const CUBE_FRAG: &str = "#version 300 es
precision highp float;
precision highp sampler2D;
uniform sampler2D kagegami_equirect;
uniform int kagegami_face;
uniform float kagegami_size;
out vec4 kagegami_FragColor;
const float PI = 3.14159265358979;
void main() {
    vec2 st = gl_FragCoord.xy / kagegami_size * 2.0 - 1.0;
    float sc = st.x;
    float tc = st.y;
    vec3 d;
    if (kagegami_face == 0) d = vec3(1.0, -tc, -sc);
    else if (kagegami_face == 1) d = vec3(-1.0, -tc, sc);
    else if (kagegami_face == 2) d = vec3(sc, 1.0, tc);
    else if (kagegami_face == 3) d = vec3(sc, -1.0, -tc);
    else if (kagegami_face == 4) d = vec3(sc, -tc, 1.0);
    else d = vec3(-sc, -tc, -1.0);
    d = normalize(d);
    float lon = atan(d.x, -d.z);
    float lat = asin(clamp(d.y, -1.0, 1.0));
    vec2 uv = vec2(lon / (2.0 * PI) + 0.5, 0.5 - lat / PI);
    kagegami_FragColor = texture(kagegami_equirect, uv);
}
";

/// 正距円筒図法の画像 `img` から、面のサイズが `face` の RGBA8 のキューブマップを作る（#68）。
/// 画像をいったん 2D のテクスチャーにし、GPU で 6 面に描いてから消す。mipmap ならその後に作る。
/// `face` は `cubemap_face_size` で確かめた値を渡す。
pub fn upload_cubemap(
    gl: &glow::Context,
    img: &Image,
    face: u32,
    sampler: Sampler,
) -> Result<Texture, String> {
    let program = crate::gl_program::build(
        gl,
        &[
            (glow::VERTEX_SHADER, CUBE_VERT),
            (glow::FRAGMENT_SHADER, CUBE_FRAG),
        ],
        |_| {},
    )
    .map_err(|e| format!("cannot create the cubemap converter: {e:?}"))?;
    drain_errors(gl);
    unsafe {
        let blend = gl.is_enabled(glow::BLEND);
        let mut viewport = [0i32; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        let mut created = (None, None, None, None);
        let result = (|| -> Result<glow::Texture, String> {
            // 元の画像。経度の方向は端がつながるので repeat、緯度の方向は clamp にする。
            let src = gl.create_texture()?;
            created.0 = Some(src);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(src));
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
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );

            let cube = gl.create_texture()?;
            created.1 = Some(cube);
            gl.bind_texture(glow::TEXTURE_CUBE_MAP, Some(cube));
            for i in 0..6 {
                gl.tex_image_2d(
                    glow::TEXTURE_CUBE_MAP_POSITIVE_X + i,
                    0,
                    glow::RGBA8 as i32,
                    face as i32,
                    face as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
            }
            gl.bind_texture(glow::TEXTURE_CUBE_MAP, None);
            if gl.get_error() != glow::NO_ERROR {
                return Err("cannot create a cubemap texture".into());
            }

            let fbo = gl.create_framebuffer()?;
            created.2 = Some(fbo);
            let vao = gl.create_vertex_array()?;
            created.3 = Some(vao);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.bind_vertex_array(Some(vao));
            gl.use_program(Some(program));
            gl.uniform_1_i32(
                gl.get_uniform_location(program, "kagegami_equirect")
                    .as_ref(),
                0,
            );
            gl.uniform_1_f32(
                gl.get_uniform_location(program, "kagegami_size").as_ref(),
                face as f32,
            );
            let face_loc = gl.get_uniform_location(program, "kagegami_face");
            gl.viewport(0, 0, face as i32, face as i32);
            gl.disable(glow::BLEND);
            gl.bind_texture(glow::TEXTURE_2D, Some(src));
            for i in 0..6 {
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_CUBE_MAP_POSITIVE_X + i,
                    Some(cube),
                    0,
                );
                let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
                if status != glow::FRAMEBUFFER_COMPLETE {
                    return Err(format!(
                        "cannot draw to a cubemap face (framebuffer status 0x{status:X})"
                    ));
                }
                gl.uniform_1_i32(face_loc.as_ref(), i as i32);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            Ok(cube)
        })();

        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_vertex_array(None);
        gl.use_program(None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
        if blend {
            gl.enable(glow::BLEND);
        }
        let (src, cube, fbo, vao) = created;
        if let Some(t) = src {
            gl.delete_texture(t);
        }
        if let Some(f) = fbo {
            gl.delete_framebuffer(f);
        }
        if let Some(v) = vao {
            gl.delete_vertex_array(v);
        }
        gl.delete_program(program);
        let cube = match result {
            Ok(c) => c,
            Err(e) => {
                if let Some(c) = cube {
                    gl.delete_texture(c);
                }
                return Err(e);
            }
        };
        gl.bind_texture(glow::TEXTURE_CUBE_MAP, Some(cube));
        set_sampler(gl, glow::TEXTURE_CUBE_MAP, sampler);
        gl.bind_texture(glow::TEXTURE_CUBE_MAP, None);
        finish(
            gl,
            Texture {
                tex: cube,
                target: glow::TEXTURE_CUBE_MAP,
                width: face,
                height: face,
                depth: 1,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cubemap_face_is_a_quarter_of_the_width() {
        assert_eq!(cubemap_face_size(2048, 1024, 16384), Ok(512));
        assert_eq!(cubemap_face_size(4, 2, 16384), Ok(1));
        // 4 で割り切れない幅は切り捨てる。
        assert_eq!(cubemap_face_size(2050, 1025, 16384), Ok(512));
        // 面がちょうど上限なら作れる。
        assert_eq!(cubemap_face_size(8192, 4096, 2048), Ok(2048));
    }

    #[test]
    fn bad_cubemap_sizes_are_errors() {
        for (w, h, want) in [
            (2048, 1023, "2:1 aspect ratio (got 2048x1023)"),
            (1024, 1024, "2:1 aspect ratio (got 1024x1024)"),
            (1024, 2048, "2:1 aspect ratio"),
            (2, 1, "too small for a cubemap (2x1"),
            (
                8196,
                4098,
                "cubemap faces are too large (2049x2049 from 8196x4098, max 2048x2048",
            ),
        ] {
            let e = cubemap_face_size(w, h, 2048).unwrap_err();
            assert!(e.contains(want), "{w}x{h}: {e}");
        }
    }
}

/// GPU を使うテスト。EGL の surfaceless のディスプレイ（Mesa）が使えなければ、何もせずに通す。
#[cfg(test)]
mod gpu_tests {
    use super::*;
    use khronos_egl as egl;

    const PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;

    /// surfaceless の GLES 3 のコンテキストを作って current にする。
    fn context() -> Option<(egl::Instance<egl::Static>, glow::Context)> {
        let egl = egl::Instance::new(egl::Static);
        let display = unsafe {
            egl.get_platform_display(
                PLATFORM_SURFACELESS_MESA,
                egl::DEFAULT_DISPLAY,
                &[egl::ATTRIB_NONE],
            )
        }
        .ok()?;
        egl.initialize(display).ok()?;
        egl.bind_api(egl::OPENGL_ES_API).ok()?;
        let config = egl
            .choose_first_config(
                display,
                // 既定の SURFACE_TYPE は WINDOW_BIT で、surfaceless のディスプレイには合う設定がない。
                &[
                    egl::SURFACE_TYPE,
                    0,
                    egl::RENDERABLE_TYPE,
                    egl::OPENGL_ES3_BIT,
                    egl::NONE,
                ],
            )
            .ok()??;
        let ctx = egl
            .create_context(
                display,
                config,
                None,
                &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE],
            )
            .ok()?;
        egl.make_current(display, None, None, Some(ctx)).ok()?;
        let gl = unsafe {
            glow::Context::from_loader_function(|s| {
                egl.get_proc_address(s)
                    .map_or(std::ptr::null(), |f| f as *const std::ffi::c_void)
            })
        };
        Some((egl, gl))
    }

    /// キューブマップを、#68 で Shadertoy と照らし合わせた式で正距円筒図法に広げて描く。
    const UNWRAP_FRAG: &str = "#version 300 es
precision highp float;
precision highp samplerCube;
uniform samplerCube cube;
uniform vec2 size;
out vec4 o;
void main() {
    vec2 uv = gl_FragCoord.xy / size;
    float lon = (uv.x - 0.5) * 6.2831853;
    float lat = (uv.y - 0.5) * 3.1415927;
    vec3 dir = vec3(sin(lon) * cos(lat), sin(lat), -cos(lon) * cos(lat));
    o = texture(cube, dir);
}
";

    /// 左右・上下が非対称な画像を、キューブマップにしてから正距円筒図法に広げ直すと、元の画像に戻る。
    /// キューブマップを引くのはドライバーの `texture(samplerCube, dir)` なので、変換の面の表の誤り
    /// （面の取り違え・反転・回転）は打ち消されずに残る。
    #[test]
    fn equirect_round_trips_through_the_cubemap() {
        let Some((_egl, gl)) = context() else {
            eprintln!("skipped: no surfaceless EGL display");
            return;
        };
        let (w, h) = (256u32, 128u32);
        // r: 左端 0 → 右端 255（経度）、g: 上端 255 → 下端 0（緯度）、b: 経度で 2 周する波。
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let u = (x as f32 + 0.5) / w as f32;
                let v = (y as f32 + 0.5) / h as f32;
                let b = 127.5 + 127.5 * (u * 4.0 * std::f32::consts::PI).sin();
                rgba.extend([(u * 255.0) as u8, ((1.0 - v) * 255.0) as u8, b as u8, 255]);
            }
        }
        let img = Image {
            width: w,
            height: h,
            rgba: rgba.clone(),
        };
        let face = cubemap_face_size(w, h, max_cube_size(&gl)).unwrap();
        let sampler = Sampler {
            filter: Filter::Linear,
            wrap: Wrap::Clamp,
            vflip: false,
        };
        // mipmap でも作れる（ミップマップは 6 面を描いた後に作る）。
        let mipmapped = upload_cubemap(
            &gl,
            &img,
            face,
            Sampler {
                filter: Filter::Mipmap,
                ..sampler
            },
        )
        .unwrap();
        delete(&gl, mipmapped);
        let cube = upload_cubemap(&gl, &img, face, sampler).unwrap();
        assert_eq!((cube.width, cube.height, cube.depth), (64, 64, 1));
        assert_eq!(cube.target, glow::TEXTURE_CUBE_MAP);

        let out = unsafe {
            let program = crate::gl_program::build(
                &gl,
                &[
                    (glow::VERTEX_SHADER, CUBE_VERT),
                    (glow::FRAGMENT_SHADER, UNWRAP_FRAG),
                ],
                |_| {},
            )
            .unwrap();
            let target = gl.create_texture().unwrap();
            gl.bind_texture(glow::TEXTURE_2D, Some(target));
            gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, w as i32, h as i32);
            let fbo = gl.create_framebuffer().unwrap();
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(target),
                0,
            );
            let vao = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(vao));
            gl.use_program(Some(program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_CUBE_MAP, Some(cube.tex));
            gl.uniform_1_i32(gl.get_uniform_location(program, "cube").as_ref(), 0);
            gl.uniform_2_f32(
                gl.get_uniform_location(program, "size").as_ref(),
                w as f32,
                h as f32,
            );
            gl.viewport(0, 0, w as i32, h as i32);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            let mut out = vec![0u8; (w * h * 4) as usize];
            gl.read_pixels(
                0,
                0,
                w as i32,
                h as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut out)),
            );
            assert_eq!(gl.get_error(), glow::NO_ERROR);
            out
        };

        // 読み出した行 0 は下端。元の画像の行 0 は上端。
        // 経度の継ぎ目の近く（r が 0 と 255 の間で跳ぶ）と、画素の細かさが大きく変わる極の近くは除く。
        let mut worst = 0;
        let mut checked = 0;
        for y in 0..h {
            let lat = ((y as f32 + 0.5) / h as f32 - 0.5) * 180.0;
            if lat.abs() > 60.0 {
                continue;
            }
            for x in 0..w {
                let u = (x as f32 + 0.5) / w as f32;
                if !(0.05..0.95).contains(&u) {
                    continue;
                }
                let o = ((y * w + x) * 4) as usize;
                let s = (((h - 1 - y) * w + x) * 4) as usize;
                for c in 0..3 {
                    let d = (out[o + c] as i32 - rgba[s + c] as i32).abs();
                    worst = worst.max(d);
                }
                checked += 1;
            }
        }
        eprintln!(
            "renderer: {}, checked {checked} pixels, max difference {worst}",
            unsafe { gl.get_parameter_string(glow::RENDERER) }
        );
        assert!(checked > 10000, "{checked}");
        assert!(worst <= 3, "max difference {worst}");
    }
}

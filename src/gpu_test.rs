//! GPU を使うテストの共通の処理。EGL の surfaceless のディスプレイ（Mesa）で、画面なしで描く。

use glow::HasContext;
use khronos_egl as egl;

const PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31DD;

/// surfaceless の GLES 3 のコンテキストを作って current にする。作れなければ None。
pub fn context() -> Option<(egl::Instance<egl::Static>, glow::Context)> {
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

/// 左右・上下が非対称な正距円筒図法の画像（RGBA8、上の行から）。
/// r: 左端 0 → 右端 255（経度）、g: 上端 255 → 下端 0（緯度）、b: 経度で 2 周する波（継ぎ目でつながる）。
pub fn panorama(w: u32, h: u32) -> Vec<u8> {
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            let b = 127.5 + 127.5 * (u * 4.0 * std::f32::consts::PI).sin();
            rgba.extend([(u * 255.0) as u8, ((1.0 - v) * 255.0) as u8, b as u8, 255]);
        }
    }
    rgba
}

/// キューブマップを、#68 で Shadertoy と照らし合わせた式で正距円筒図法に広げて描く。
pub const UNWRAP_FRAG: &str = "#version 300 es
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

/// `size` の RGBA8 のテクスチャーに `draw` で描き、読み出す（行 0 が下端）。
pub fn render(gl: &glow::Context, size: (u32, u32), draw: impl FnOnce()) -> Vec<u8> {
    let (w, h) = size;
    unsafe {
        let target = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(target));
        gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, w as i32, h as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);
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
        gl.viewport(0, 0, w as i32, h as i32);
        draw();
        // draw が FBO を外していても読めるように、もう一度つなぐ。
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
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
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.delete_framebuffer(fbo);
        gl.delete_texture(target);
        gl.delete_vertex_array(vao);
        out
    }
}

/// `UNWRAP_FRAG` の式で広げた `out`（行 0 が下端）が、元の `panorama`（行 0 が上端）と合うか確かめる。
/// r（経度）は継ぎ目で 0 と 255 の間を跳ぶので、継ぎ目の近くでは比べない。g と b は継ぎ目でもつながるので
/// すべての列で比べる。画素の細かさが大きく変わる極の近く（緯度 60° より上下）は比べない。
pub fn assert_unwrapped(gl: &glow::Context, out: &[u8], panorama: &[u8], size: (u32, u32)) {
    let (w, h) = size;
    let mut worst = 0;
    let mut checked = 0;
    for y in 0..h {
        let lat = ((y as f32 + 0.5) / h as f32 - 0.5) * 180.0;
        if lat.abs() > 60.0 {
            continue;
        }
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let first = if (0.05..0.95).contains(&u) { 0 } else { 1 };
            let o = ((y * w + x) * 4) as usize;
            let s = (((h - 1 - y) * w + x) * 4) as usize;
            for c in first..3 {
                let d = (out[o + c] as i32 - panorama[s + c] as i32).abs();
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

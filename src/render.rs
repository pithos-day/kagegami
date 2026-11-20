//! GLES2 による描画。

use glow::HasContext;

use crate::egl::EglContext;

pub struct Renderer {
    gl: glow::Context,
}

impl Renderer {
    pub fn new(egl: &EglContext) -> Self {
        let gl = unsafe { glow::Context::from_loader_function(|s| egl.proc_address(s)) };
        Self { gl }
    }

    pub fn begin(&mut self, width: u32, height: u32) {
        unsafe { self.gl.viewport(0, 0, width as i32, height as i32) };
    }

    pub fn clear(&mut self, r: f32, g: f32, b: f32) {
        unsafe {
            self.gl.clear_color(r, g, b, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    pub fn end(&mut self) {}
}

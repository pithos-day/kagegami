//! EGL の初期化と、wl_surface ごとの描画先の管理。

use std::ffi::c_void;

use khronos_egl as egl;
use wayland_egl::WlEglSurface;

pub type Egl = egl::Instance<egl::Static>;

pub struct EglContext {
    pub egl: Egl,
    pub display: egl::Display,
    pub config: egl::Config,
    pub context: egl::Context,
}

impl EglContext {
    /// `wl_display` は libwayland の生ポインタ（wayland-backend の client_system で取得）。
    pub fn new(wl_display: *mut c_void) -> Result<Self, String> {
        let egl = egl::Instance::new(egl::Static);
        let display = unsafe { egl.get_display(wl_display) }.ok_or("eglGetDisplay failed")?;
        egl.initialize(display)
            .map_err(|e| format!("eglInitialize: {e}"))?;
        egl.bind_api(egl::OPENGL_ES_API)
            .map_err(|e| format!("eglBindAPI: {e}"))?;

        let attrs = [
            egl::SURFACE_TYPE,
            egl::WINDOW_BIT,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES2_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::NONE,
        ];
        let config = egl
            .choose_first_config(display, &attrs)
            .map_err(|e| format!("eglChooseConfig: {e}"))?
            .ok_or("no suitable EGL config")?;
        let ctx_attrs = [egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE];
        let context = egl
            .create_context(display, config, None, &ctx_attrs)
            .map_err(|e| format!("eglCreateContext: {e}"))?;
        Ok(Self {
            egl,
            display,
            config,
            context,
        })
    }

    pub fn create_surface(
        &self,
        wl_surface: &wayland_client::protocol::wl_surface::WlSurface,
        w: i32,
        h: i32,
    ) -> Result<EglSurface, String> {
        use wayland_client::Proxy;
        let window =
            WlEglSurface::new(wl_surface.id(), w, h).map_err(|e| format!("wl_egl_window: {e}"))?;
        let surface = unsafe {
            self.egl.create_window_surface(
                self.display,
                self.config,
                window.ptr() as *mut c_void,
                None,
            )
        }
        .map_err(|e| format!("eglCreateWindowSurface: {e}"))?;
        let s = EglSurface { window, surface };
        // フレームのタイミングは wl_surface.frame で自前管理する。
        // eglSwapBuffers がブロックすると、1 つの出力の停止が全出力を止めてしまう。
        self.make_current(&s)?;
        self.egl
            .swap_interval(self.display, 0)
            .map_err(|e| format!("eglSwapInterval: {e}"))?;
        Ok(s)
    }

    pub fn make_current(&self, s: &EglSurface) -> Result<(), String> {
        self.egl
            .make_current(
                self.display,
                Some(s.surface),
                Some(s.surface),
                Some(self.context),
            )
            .map_err(|e| format!("eglMakeCurrent: {e}"))
    }

    pub fn swap(&self, s: &EglSurface) -> Result<(), String> {
        self.egl
            .swap_buffers(self.display, s.surface)
            .map_err(|e| format!("eglSwapBuffers: {e}"))
    }

    pub fn destroy_surface(&self, s: EglSurface) {
        let _ = self
            .egl
            .make_current(self.display, None, None, Some(self.context));
        let _ = self.egl.destroy_surface(self.display, s.surface);
        drop(s.window);
    }

    pub fn proc_address(&self, name: &str) -> *const c_void {
        self.egl
            .get_proc_address(name)
            .map_or(std::ptr::null(), |f| f as *const c_void)
    }
}

pub struct EglSurface {
    window: WlEglSurface,
    surface: egl::Surface,
}

impl EglSurface {
    pub fn resize(&self, w: i32, h: i32) {
        self.window.resize(w, h, 0, 0);
    }
}

//! Wayland のイベント処理と、出力ごとの壁紙 surface の管理。

use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
};
use wayland_client::{
    Connection, QueueHandle, delegate_noop,
    protocol::{wl_output, wl_pointer, wl_seat, wl_surface},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

use crate::cli::{Args, LayerName};
use crate::egl::{EglContext, EglSurface};
use crate::pointer::{self, Drag};
use crate::render::Renderer;
use crate::script::{MouseState, Script};
use crate::shader::{self, GpuTimer, ShaderProgram};

/// 出力ごとに違う乱数の種を作る。
fn seed_for<T: std::hash::Hash>(salt: &T) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    salt.hash(&mut h);
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.finish()
}

impl From<LayerName> for Layer {
    fn from(name: LayerName) -> Self {
        match name {
            LayerName::Background => Layer::Background,
            LayerName::Bottom => Layer::Bottom,
            LayerName::Top => Layer::Top,
            LayerName::Overlay => Layer::Overlay,
        }
    }
}

/// 出力ごとのエラーを、出力の名前を付けて出す。
fn report(output: &str, e: impl std::fmt::Display) {
    eprintln!("kagegami: {output}: {e}");
}

/// 描画の準備ができていなければ作る。作れなければ続けられないので終了する。
/// `App` のほかのフィールドを借りたまま呼べるように、`App` のメソッドにしない。
fn ensure_renderer<'a>(renderer: &'a mut Option<Renderer>, egl: &EglContext) -> &'a mut Renderer {
    if renderer.is_none() {
        match Renderer::new(egl) {
            Ok(r) => *renderer = Some(r),
            Err(e) => {
                eprintln!("kagegami: {e}");
                std::process::exit(1);
            }
        }
    }
    renderer.as_mut().unwrap()
}

/// 出力のピクセル数に `--scale` の倍率を掛けた、描画するバッファーのピクセル数。
fn buffer_px(output_px: u32, render_scale: f64) -> u32 {
    ((output_px as f64 * render_scale).round() as u32).max(1)
}

pub struct Wallpaper {
    output: wl_output::WlOutput,
    pub name: String,
    layer: LayerSurface,
    /// 出力の整数の倍率（`wl_output` の scale）。`--scale` の倍率とは別。
    output_scale: i32,
    /// `--scale` が 1 でないときだけ使う。バッファーを論理サイズへ拡大・縮小して表示する。
    viewport: Option<WpViewport>,
    /// layer surface の論理サイズ。
    logical_size: (u32, u32),
    /// 出力のピクセルサイズ（論理サイズ × `output_scale`）。スクリプトの座標系はこのサイズ。
    output_size: (u32, u32),
    /// 描画するバッファーのピクセルサイズ（出力のピクセルサイズ × `--scale`）。
    buffer_size: (u32, u32),
    egl_surface: Option<EglSurface>,
    /// frame callback を待っている間は描画しない（隠れている出力は自然に止まる）。
    frame_pending: bool,
    next_due: Instant,
    /// 一度でもバッファーを提示したか（layer surface は提示するまで表示されない）。
    presented: bool,
    /// 読み込んだ壁紙。まだ読み込めていなければ None。
    content: Option<Content>,
    /// 壁紙がエラーで止まっている。直前のフレームを表示したままにする。
    /// エラーで止まっても `content` は残し、読み直しに失敗しても前の `content` を使い続ける。
    failed: bool,
    last_frame: Option<Instant>,
    seed: u64,
    mouse: MouseState,
    /// シェーダーの `iMouse` と、Lua の `mouse.press_x`・`press_y`・`dragging`。
    drag: Drag,
}

/// 壁紙の中身。種類は起動時のファイルの拡張子で決まり、読み直しても変わらない。
enum Content {
    Lua(Script),
    Shader(ShaderState),
}

/// コンパイル済みのシェーダーと、`iTime`・`iFrame` の基準。
struct ShaderState {
    program: ShaderProgram,
    /// GPU 時間を測るタイマー。拡張機能がなければ None（見張らずに描く）。
    timer: Option<GpuTimer>,
    started: Instant,
    frame: i32,
}

impl ShaderState {
    /// 1 フレーム描く。前のフレームまでに測れた GPU 時間が上限を超えていたら、描かずにエラーを返す。
    fn draw(
        &mut self,
        gl: &glow::Context,
        resolution: (u32, u32),
        mouse: [f32; 4],
        now: Instant,
        dt: f64,
    ) -> Result<(), String> {
        let slow = self
            .timer
            .as_mut()
            .and_then(|t| t.poll(gl))
            .filter(|d| *d > shader::FRAME_GPU_LIMIT);
        if let Some(d) = slow {
            return Err(format!(
                "shader took {} ms on the GPU in one frame (limit {} ms)",
                d.as_millis(),
                shader::FRAME_GPU_LIMIT.as_millis()
            ));
        }
        let u = shader::Uniforms {
            resolution,
            time: now.duration_since(self.started).as_secs_f32(),
            time_delta: dt as f32,
            frame: self.frame,
            mouse,
            date: shader::local_date(),
        };
        if let Some(t) = self.timer.as_mut() {
            t.begin(gl);
        }
        self.program.draw(gl, &u);
        if let Some(t) = self.timer.as_mut() {
            t.end(gl);
        }
        self.frame = self.frame.wrapping_add(1);
        Ok(())
    }
}

/// Lua の `update` に渡す `dt` の上限（秒）。README の `update(dt)` の説明と合わせて変える。
const MAX_LUA_DT: f64 = 0.25;

/// 壁紙の中身を 1 フレーム描く。`dt` は前のフレームからの秒数。読み込めていないときや、
/// エラーで止まっているときは黒で塗る。壁紙を止めるべきエラーが起きたら、そのメッセージを返す。
fn draw_content(
    w: &mut Wallpaper,
    renderer: &mut Renderer,
    now: Instant,
    dt: f64,
) -> Result<(), String> {
    match &mut w.content {
        Some(Content::Shader(sh)) if !w.failed => {
            let mouse = shader::mouse_to_buffer_px(
                w.buffer_size,
                w.output_size,
                w.drag.imouse(w.output_size.1 as f32),
            );
            sh.draw(renderer.gl(), w.buffer_size, mouse, now, dt)
        }
        Some(Content::Lua(script)) if !w.failed => {
            let dl = script.frame(dt.min(MAX_LUA_DT))?;
            renderer.submit(&dl);
            Ok(())
        }
        _ => {
            renderer.clear(0.0, 0.0, 0.0);
            Ok(())
        }
    }
}

impl Wallpaper {
    fn surface(&self) -> &wl_surface::WlSurface {
        self.layer.wl_surface()
    }
}

pub struct App {
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub seat_state: SeatState,
    pub pointer: Option<wl_pointer::WlPointer>,
    pub compositor: CompositorState,
    pub layer_shell: LayerShell,
    pub viewporter: Option<WpViewporter>,
    pub egl: EglContext,
    pub renderer: Option<Renderer>,
    /// タイマーの拡張機能がないことを一度警告したか。
    pub timer_warned: bool,
    pub args: Args,
    pub frame_period: Duration,
    pub wallpapers: Vec<Wallpaper>,
    #[cfg(feature = "gpu-bench")]
    pub bench: Option<crate::bench::Bench>,
}

impl App {
    fn add_output(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let Some(info) = self.output_state.info(&output) else {
            return;
        };
        let Some(name) = info.name.clone() else {
            return;
        };
        if !self.args.output.matches(&name) || self.wallpapers.iter().any(|w| w.output == output) {
            return;
        }
        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            self.args.layer.into(),
            Some("kagegami"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        let output_scale = info.scale_factor.max(1);
        let viewport = self
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(layer.wl_surface(), qh, ()));
        // viewport を使うときは、表示するサイズを set_destination で決める。
        if viewport.is_none() {
            layer.wl_surface().set_buffer_scale(output_scale);
        }
        layer.commit();
        eprintln!("kagegami: attached to output {name}");
        self.wallpapers.push(Wallpaper {
            output,
            name,
            layer,
            output_scale,
            viewport,
            logical_size: (0, 0),
            output_size: (0, 0),
            buffer_size: (0, 0),
            egl_surface: None,
            frame_pending: false,
            next_due: Instant::now(),
            presented: false,
            content: None,
            failed: false,
            last_frame: None,
            seed: seed_for(&self.wallpapers.len()),
            mouse: MouseState::default(),
            drag: Drag::default(),
        });
    }

    fn is_shader(&self) -> bool {
        shader::is_shader_path(&self.args.script)
    }

    /// シェーダーのファイルを読んでコンパイルし、i 番目の出力の壁紙にする。
    /// 失敗したら、それまでのシェーダーをそのまま残してエラーを返す（Lua の読み直しと同じ）。
    fn load_shader(&mut self, i: usize) -> Result<(), String> {
        let src = std::fs::read_to_string(&self.args.script)
            .map_err(|e| format!("{}: {e}", self.args.script.display()))?;
        let es = self.wallpapers[i]
            .egl_surface
            .as_ref()
            .ok_or("no EGL surface")?;
        self.egl.make_current(es)?;
        let gl = ensure_renderer(&mut self.renderer, &self.egl).gl();
        let program = ShaderProgram::new(gl, &src)?;
        let old = match self.wallpapers[i].content.take() {
            Some(Content::Shader(old)) => Some(old),
            _ => None,
        };
        let timer = match old {
            Some(old) => {
                old.program.delete(gl);
                old.timer.map(|mut t| {
                    // 前のシェーダーの結果で、直したシェーダーを止めないようにする。
                    t.reset();
                    t
                })
            }
            None => {
                let t = GpuTimer::new(gl, |n| self.egl.proc_address(n));
                if t.is_none() && !self.timer_warned {
                    self.timer_warned = true;
                    eprintln!(
                        "kagegami: warning: GL_EXT_disjoint_timer_query is not available; \
                         shaders that take too long on the GPU will not be stopped"
                    );
                }
                t
            }
        };
        self.wallpapers[i].content = Some(Content::Shader(ShaderState {
            program,
            timer,
            started: Instant::now(),
            frame: 0,
        }));
        Ok(())
    }

    /// 壁紙のファイルを読み、i 番目の出力の壁紙にする。`seed` は Lua の乱数の種。
    /// 失敗したら、それまでの壁紙をそのまま残してエラーを返す。
    fn load_content(&mut self, i: usize, seed: u64) -> Result<(), String> {
        if self.is_shader() {
            return self.load_shader(i);
        }
        let w = &mut self.wallpapers[i];
        let (width, height) = w.output_size;
        let script = Script::load(&self.args.script, &w.name, seed, width, height)?;
        w.content = Some(Content::Lua(script));
        Ok(())
    }

    /// i 番目の出力で壁紙を読み込めるか。シェーダーはコンパイルに EGL surface が要る。
    /// Lua は出力のサイズが決まっていればよい。
    fn can_load(&self, i: usize) -> bool {
        let w = &self.wallpapers[i];
        if self.is_shader() {
            w.egl_surface.is_some()
        } else {
            w.output_size.0 != 0
        }
    }

    fn find(&mut self, surface: &wl_surface::WlSurface) -> Option<&mut Wallpaper> {
        self.wallpapers.iter_mut().find(|w| w.surface() == surface)
    }

    /// 壁紙のファイルを読み直す。読み込めた出力だけ新しい壁紙に切り替える。
    pub fn reload(&mut self) {
        eprintln!("kagegami: reloading {}", self.args.script.display());
        for i in 0..self.wallpapers.len() {
            if !self.can_load(i) {
                continue;
            }
            // 読み直すたびに新しい種を使う（#26）。README の「壁紙の書き方」と合わせて変える。
            let seed = seed_for(&self.wallpapers[i].seed);
            match self.load_content(i, seed) {
                Ok(()) => {
                    let w = &mut self.wallpapers[i];
                    w.failed = false;
                    w.last_frame = None;
                    w.next_due = Instant::now();
                }
                Err(e) => report(&self.wallpapers[i].name, e),
            }
        }
    }

    /// 次に描画すべき時刻までの待ち時間。描画できる出力がなければ None。
    pub fn next_timeout(&self, now: Instant) -> Option<Duration> {
        self.wallpapers
            .iter()
            // エラーで止まっている出力は描き直さないので、待つ必要もない。
            .filter(|w| w.egl_surface.is_some() && !w.frame_pending && !(w.failed && w.presented))
            .map(|w| w.next_due.saturating_duration_since(now))
            .min()
    }

    pub fn render_due(&mut self, qh: &QueueHandle<Self>, now: Instant) {
        for i in 0..self.wallpapers.len() {
            let w = &mut self.wallpapers[i];
            if w.frame_pending || now < w.next_due || w.egl_surface.is_none() {
                continue;
            }
            // 遅れた場合は追いつこうとせず、今から 1 周期後にする。
            w.next_due = (w.next_due + self.frame_period).max(now);
            // エラーで止まった壁紙は、直前のフレームを表示したまま描き直さない。
            if w.failed && w.presented {
                continue;
            }
            self.render(i, qh, now);
        }
    }

    /// i 番目の出力の壁紙を 1 フレーム描いて提示する。
    fn render(&mut self, i: usize, qh: &QueueHandle<Self>, now: Instant) {
        let w = &mut self.wallpapers[i];
        if let Some(Content::Lua(script)) = &w.content
            && let Err(e) = script.set_mouse(&w.mouse)
        {
            report(&w.name, e);
        }
        let dt = w
            .last_frame
            .map_or(0.0, |t| now.duration_since(t).as_secs_f64());
        w.last_frame = Some(now);

        if let Err(e) = self.egl.make_current(w.egl_surface.as_ref().unwrap()) {
            report(&w.name, e);
            return;
        }
        let renderer = ensure_renderer(&mut self.renderer, &self.egl);
        #[cfg(feature = "gpu-bench")]
        let bench_t0 = self.bench.as_ref().map(|b| b.start(renderer.gl()));
        renderer.begin(w.buffer_size, w.output_size);
        #[cfg(feature = "gpu-bench")]
        let result = match self.bench.as_mut() {
            // 比較用のシェーダーは、壁紙がエラーで止まっていても描く。
            Some(b) if b.uses_glsl() => {
                b.draw_glsl(renderer.gl(), w.buffer_size.0, w.buffer_size.1);
                Ok(())
            }
            _ => draw_content(w, renderer, now, dt),
        };
        #[cfg(not(feature = "gpu-bench"))]
        let result = draw_content(w, renderer, now, dt);
        let failed = match result {
            Ok(()) => false,
            Err(e) => {
                report(&w.name, e);
                renderer.clear(0.0, 0.0, 0.0);
                true
            }
        };
        #[cfg(feature = "gpu-bench")]
        if let (Some(b), Some(t0)) = (self.bench.as_mut(), bench_t0) {
            let label = if b.uses_glsl() {
                "glsl"
            } else if matches!(w.content, Some(Content::Shader(_))) {
                "shader"
            } else {
                "lua"
            };
            b.end(renderer.gl(), t0, label);
        }
        if failed && w.presented {
            // 直前のフレームを表示したまま止める。
            w.failed = true;
            return;
        }
        #[cfg(debug_assertions)]
        if let Ok(dir) = std::env::var("KAGEGAMI_DUMP_DIR") {
            let path = format!("{dir}/{}.ppm", w.name);
            if !std::path::Path::new(&path).exists() {
                let _ = renderer.dump_ppm(w.buffer_size.0, w.buffer_size.1, &path);
            }
        }

        let surface = w.surface().clone();
        surface.frame(qh, FrameCallbackData(surface.clone()));
        if let Err(e) = self.egl.swap(w.egl_surface.as_ref().unwrap()) {
            report(&w.name, e);
        }
        w.frame_pending = true;
        w.presented = true;
        w.failed |= failed;
        w.drag.frame_rendered();
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(w) = self.find(surface) {
            w.frame_pending = false;
        }
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.add_output(qh, output);
    }
    fn update_output(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        // 名前が後から届いた場合に備えて再試行する。
        self.add_output(qh, output.clone());
        let output_scale = self
            .output_state
            .info(&output)
            .map_or(1, |i| i.scale_factor.max(1));
        if let Some(i) = self.wallpapers.iter().position(|w| w.output == output) {
            let w = &mut self.wallpapers[i];
            if w.output_scale != output_scale {
                w.output_scale = output_scale;
                if w.viewport.is_none() {
                    w.layer.wl_surface().set_buffer_scale(output_scale);
                    w.layer.commit();
                } else if w.logical_size != (0, 0) {
                    // viewport を使うときは論理サイズが変わらないので configure が来ない。
                    self.apply_size(i);
                }
            }
        }
    }
    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let Some(i) = self.wallpapers.iter().position(|w| w.output == output) {
            let mut w = self.wallpapers.remove(i);
            eprintln!("kagegami: output {} removed", w.name);
            if let Some(s) = w.egl_surface.take() {
                self.egl.destroy_surface(s);
            }
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(i) = self.wallpapers.iter().position(|w| &w.layer == layer) {
            let mut w = self.wallpapers.remove(i);
            if let Some(s) = w.egl_surface.take() {
                self.egl.destroy_surface(s);
            }
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(i) = self.wallpapers.iter().position(|w| &w.layer == layer) else {
            return;
        };
        let (lw, lh) = configure.new_size;
        if lw == 0 || lh == 0 {
            return;
        }
        self.wallpapers[i].logical_size = (lw, lh);
        self.apply_size(i);
    }
}

impl App {
    /// 論理サイズと出力の倍率から各サイズを決め直し、EGL surface とスクリプトに反映する。
    fn apply_size(&mut self, i: usize) {
        let w = &mut self.wallpapers[i];
        let (lw, lh) = w.logical_size;
        let (ow, oh) = (lw * w.output_scale as u32, lh * w.output_scale as u32);
        w.output_size = (ow, oh);
        w.buffer_size = match &w.viewport {
            Some(vp) => {
                vp.set_destination(lw as i32, lh as i32);
                let render_scale = self.args.render_scale;
                (buffer_px(ow, render_scale), buffer_px(oh, render_scale))
            }
            None => (ow, oh),
        };
        let (bw, bh) = (w.buffer_size.0 as i32, w.buffer_size.1 as i32);
        match &w.egl_surface {
            Some(s) => s.resize(bw, bh),
            None => match self.egl.create_surface(w.layer.wl_surface(), bw, bh) {
                Ok(s) => w.egl_surface = Some(s),
                Err(e) => report(&w.name, e),
            },
        }
        let w = &mut self.wallpapers[i];
        // 設定が変わったらすぐに描き直す。
        w.frame_pending = false;
        w.next_due = Instant::now();
        if let Some(Content::Lua(script)) = &w.content
            && let Err(e) = script.set_screen(w.output_size.0, w.output_size.1)
        {
            report(&w.name, e);
        }
        if w.content.is_none() && !w.failed && self.can_load(i) {
            let seed = self.wallpapers[i].seed;
            if let Err(e) = self.load_content(i, seed) {
                let w = &mut self.wallpapers[i];
                report(&w.name, e);
                w.failed = true;
            }
        }
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_dispatch2!(App);
delegate_registry!(App);
// wp_viewporter と wp_viewport にはイベントがない。
delegate_noop!(App: WpViewporter);
delegate_noop!(App: WpViewport);

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(p) => self.pointer = Some(p),
                Err(e) => eprintln!("kagegami: pointer: {e}"),
            }
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && let Some(p) = self.pointer.take()
        {
            p.release();
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let Some(w) = self.find(&event.surface) else {
                continue;
            };
            // Wayland の座標は論理ピクセルなので、出力のピクセルに直す。
            let x = (event.position.0 * w.output_scale as f64) as f32;
            let y = (event.position.1 * w.output_scale as f64) as f32;
            w.drag.apply(&event.kind, x, y);
            let click = pointer::apply_to_lua_mouse(&mut w.mouse, &w.drag, &event.kind, x, y);
            if let (Some(ev), Some(Content::Lua(script))) = (click, &w.content) {
                if w.failed {
                    continue;
                }
                let r = script
                    .set_mouse(&w.mouse)
                    .and_then(|_| script.mouse_event(ev));
                if let Err(e) = r {
                    report(&w.name, e);
                    w.failed = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::buffer_px;

    #[test]
    fn scales_buffer_size() {
        assert_eq!(buffer_px(3840, 1.0), 3840);
        assert_eq!(buffer_px(3840, 0.5), 1920);
        assert_eq!(buffer_px(3840, 0.25), 960);
        assert_eq!(buffer_px(2160, 2.0), 4320);
        // 割り切れないときは四捨五入する。
        assert_eq!(buffer_px(1366, 0.25), 342);
        assert_eq!(buffer_px(1, 0.25), 1);
    }
}

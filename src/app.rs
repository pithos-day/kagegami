//! Wayland のイベント処理と、出力ごとの壁紙 surface の管理。

use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
};
use wayland_client::{
    Connection, QueueHandle,
    protocol::{wl_output, wl_surface},
};

use crate::cli::Args;
use crate::egl::{EglContext, EglSurface};
use crate::render::Renderer;

pub struct Wallpaper {
    output: wl_output::WlOutput,
    pub name: String,
    layer: LayerSurface,
    scale: i32,
    /// バッファーのピクセルサイズ（論理サイズ × scale）。
    width: u32,
    height: u32,
    egl_surface: Option<EglSurface>,
    /// frame callback を待っている間は描画しない（隠れている出力は自然に止まる）。
    frame_pending: bool,
    next_due: Instant,
}

impl Wallpaper {
    fn surface(&self) -> &wl_surface::WlSurface {
        self.layer.wl_surface()
    }
}

pub struct App {
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub compositor: CompositorState,
    pub layer_shell: LayerShell,
    pub egl: EglContext,
    pub renderer: Option<Renderer>,
    pub args: Args,
    pub frame_period: Duration,
    pub wallpapers: Vec<Wallpaper>,
}

impl App {
    fn add_output(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let Some(info) = self.output_state.info(&output) else { return };
        let Some(name) = info.name.clone() else { return };
        if !self.args.output.matches(&name) || self.wallpapers.iter().any(|w| w.output == output) {
            return;
        }
        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Background,
            Some("kagegami"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        let scale = info.scale_factor.max(1);
        layer.wl_surface().set_buffer_scale(scale);
        layer.commit();
        eprintln!("kagegami: attached to output {name}");
        self.wallpapers.push(Wallpaper {
            output,
            name,
            layer,
            scale,
            width: 0,
            height: 0,
            egl_surface: None,
            frame_pending: false,
            next_due: Instant::now(),
        });
    }

    fn find(&mut self, surface: &wl_surface::WlSurface) -> Option<&mut Wallpaper> {
        self.wallpapers.iter_mut().find(|w| w.surface() == surface)
    }

    /// 次に描画すべき時刻までの待ち時間。描画できる出力がなければ None。
    pub fn next_timeout(&self, now: Instant) -> Option<Duration> {
        self.wallpapers
            .iter()
            .filter(|w| w.egl_surface.is_some() && !w.frame_pending)
            .map(|w| w.next_due.saturating_duration_since(now))
            .min()
    }

    pub fn render_due(&mut self, qh: &QueueHandle<Self>, now: Instant) {
        for i in 0..self.wallpapers.len() {
            let w = &self.wallpapers[i];
            if w.frame_pending || now < w.next_due {
                continue;
            }
            let Some(es) = w.egl_surface.as_ref() else { continue };
            if let Err(e) = self.egl.make_current(es) {
                eprintln!("kagegami: {}: {e}", w.name);
                continue;
            }
            let renderer = self.renderer.get_or_insert_with(|| Renderer::new(&self.egl));
            renderer.begin(w.width, w.height);
            renderer.clear(0.039, 0.039, 0.039);
            renderer.end();

            let surface = w.surface().clone();
            surface.frame(qh, FrameCallbackData(surface.clone()));
            if let Err(e) = self.egl.swap(es) {
                eprintln!("kagegami: {}: {e}", w.name);
            }
            let w = &mut self.wallpapers[i];
            w.frame_pending = true;
            // 遅れた場合は追いつこうとせず、今から 1 周期後にする。
            w.next_due = (w.next_due + self.frame_period).max(now);
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        if let Some(w) = self.find(surface) {
            w.frame_pending = false;
        }
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.add_output(qh, output);
    }
    fn update_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        // 名前が後から届いた場合に備えて再試行する。
        self.add_output(qh, output.clone());
        let scale = self.output_state.info(&output).map_or(1, |i| i.scale_factor.max(1));
        if let Some(w) = self.wallpapers.iter_mut().find(|w| w.output == output) {
            if w.scale != scale {
                w.scale = scale;
                w.layer.wl_surface().set_buffer_scale(scale);
                w.layer.commit();
            }
        }
    }
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
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

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, configure: LayerSurfaceConfigure, _: u32) {
        let Some(i) = self.wallpapers.iter().position(|w| &w.layer == layer) else { return };
        let w = &mut self.wallpapers[i];
        let (lw, lh) = configure.new_size;
        if lw == 0 || lh == 0 {
            return;
        }
        w.width = lw * w.scale as u32;
        w.height = lh * w.scale as u32;
        match &w.egl_surface {
            Some(s) => s.resize(w.width as i32, w.height as i32),
            None => match self.egl.create_surface(layer.wl_surface(), w.width as i32, w.height as i32) {
                Ok(s) => w.egl_surface = Some(s),
                Err(e) => eprintln!("kagegami: {}: {e}", w.name),
            },
        }
        // 設定が変わったらすぐに描き直す。
        w.frame_pending = false;
        w.next_due = Instant::now();
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_dispatch2!(App);
delegate_registry!(App);

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
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
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
    Connection, QueueHandle,
    protocol::{wl_output, wl_pointer, wl_seat, wl_surface},
};

use crate::cli::{Args, LayerName};
use crate::egl::{EglContext, EglSurface};
use crate::render::Renderer;
use crate::script::{Button, MouseEvent, MouseState, Script};

// linux/input-event-codes.h
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

/// 出力ごとに違う乱数の種を作る。
fn seed_for(index: &usize) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_usize(*index);
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.finish()
}

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
    /// 一度でもバッファーを提示したか（layer surface は提示するまで表示されない）。
    presented: bool,
    script: Option<Script>,
    /// スクリプトがエラーで止まっている。直前のフレームを表示したままにする。
    failed: bool,
    last_frame: Option<Instant>,
    seed: u64,
    mouse: MouseState,
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
            match self.args.layer {
                LayerName::Background => Layer::Background,
                LayerName::Bottom => Layer::Bottom,
                LayerName::Top => Layer::Top,
                LayerName::Overlay => Layer::Overlay,
            },
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
            presented: false,
            script: None,
            failed: false,
            last_frame: None,
            seed: seed_for(&self.wallpapers.len()),
            mouse: MouseState::default(),
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
            let w = &mut self.wallpapers[i];
            if w.frame_pending || now < w.next_due || w.egl_surface.is_none() {
                continue;
            }
            // 遅れた場合は追いつこうとせず、今から 1 周期後にする。
            w.next_due = (w.next_due + self.frame_period).max(now);
            if w.failed && w.presented {
                continue;
            }
            if let Some(script) = &w.script {
                if let Err(e) = script.set_mouse(&w.mouse) {
                    eprintln!("kagegami: {}: {e}", w.name);
                }
            }
            let dt = w.last_frame.map_or(0.0, |t| now.duration_since(t).as_secs_f64()).min(0.25);
            w.last_frame = Some(now);

            let w = &self.wallpapers[i];
            let es = w.egl_surface.as_ref().unwrap();
            if let Err(e) = self.egl.make_current(es) {
                eprintln!("kagegami: {}: {e}", w.name);
                continue;
            }
            if self.renderer.is_none() {
                match Renderer::new(&self.egl) {
                    Ok(r) => self.renderer = Some(r),
                    Err(e) => {
                        eprintln!("kagegami: {e}");
                        std::process::exit(1);
                    }
                }
            }
            let renderer = self.renderer.as_mut().unwrap();
            renderer.begin(w.width, w.height);
            let mut failed = false;
            match &w.script {
                Some(script) if !w.failed => match script.frame(dt) {
                    Ok(dl) => renderer.submit(&dl),
                    Err(e) => {
                        eprintln!("kagegami: {}: {e}", w.name);
                        failed = true;
                        renderer.clear(0.0, 0.0, 0.0);
                    }
                },
                _ => renderer.clear(0.0, 0.0, 0.0),
            }
            if failed && w.presented {
                // 直前のフレームを表示したまま止める。
                self.wallpapers[i].failed = true;
                continue;
            }
            #[cfg(debug_assertions)]
            if let Ok(dir) = std::env::var("KAGEGAMI_DUMP_DIR") {
                let path = format!("{dir}/{}.ppm", w.name);
                if !std::path::Path::new(&path).exists() {
                    let _ = renderer.dump_ppm(w.width, w.height, &path);
                }
            }

            let surface = w.surface().clone();
            surface.frame(qh, FrameCallbackData(surface.clone()));
            if let Err(e) = self.egl.swap(es) {
                eprintln!("kagegami: {}: {e}", w.name);
            }
            let w = &mut self.wallpapers[i];
            w.frame_pending = true;
            w.presented = true;
            w.failed |= failed;
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
        match &w.script {
            Some(script) => {
                if let Err(e) = script.set_screen(w.width, w.height) {
                    eprintln!("kagegami: {}: {e}", w.name);
                }
            }
            None if !w.failed => match Script::load(&self.args.script, &w.name, w.seed, w.width, w.height) {
                Ok(s) => w.script = Some(s),
                Err(e) => {
                    eprintln!("kagegami: {}: {e}", w.name);
                    w.failed = true;
                }
            },
            None => {}
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
    registry_handlers![OutputState, SeatState];
}

delegate_dispatch2!(App);
delegate_registry!(App);

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(p) => self.pointer = Some(p),
                Err(e) => eprintln!("kagegami: pointer: {e}"),
            }
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for event in events {
            let Some(w) = self.find(&event.surface) else { continue };
            // Wayland の座標は論理ピクセルなので、バッファーのピクセルに直す。
            let x = (event.position.0 * w.scale as f64) as f32;
            let y = (event.position.1 * w.scale as f64) as f32;
            let mut click = None;
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    w.mouse.inside = true;
                }
                PointerEventKind::Leave { .. } => {
                    // カーソルが外に出たら、押していたボタンの状態も分からなくなる。
                    w.mouse = MouseState { x: w.mouse.x, y: w.mouse.y, ..Default::default() };
                    continue;
                }
                PointerEventKind::Press { button, .. } | PointerEventKind::Release { button, .. } => {
                    let pressed = matches!(event.kind, PointerEventKind::Press { .. });
                    let b = match button {
                        BTN_LEFT => Button::Left,
                        BTN_RIGHT => Button::Right,
                        BTN_MIDDLE => Button::Middle,
                        _ => continue,
                    };
                    *match b {
                        Button::Left => &mut w.mouse.left,
                        Button::Right => &mut w.mouse.right,
                        Button::Middle => &mut w.mouse.middle,
                    } = pressed;
                    click = Some(if pressed { MouseEvent::Pressed(x, y, b) } else { MouseEvent::Released(x, y, b) });
                }
                PointerEventKind::Axis { .. } => continue,
            }
            w.mouse.x = x;
            w.mouse.y = y;
            if let (Some(ev), Some(script)) = (click, &w.script) {
                if w.failed {
                    continue;
                }
                let r = script.set_mouse(&w.mouse).and_then(|_| script.mouse_event(ev));
                if let Err(e) = r {
                    eprintln!("kagegami: {}: {e}", w.name);
                    w.failed = true;
                }
            }
        }
    }
}

//! Wayland のイベント処理と、出力ごとの壁紙 surface の管理。

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use glow::HasContext;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{PointerEvent, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure, SurfaceKind,
        },
    },
};
use wayland_client::{
    Connection, Proxy, QueueHandle, delegate_noop,
    globals::GlobalList,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

use crate::buffers::{self, BufferSet};
use crate::cli::{Args, LayerName};
use crate::cubemap_pass::{self, CubemapPass};
use crate::directive::{self, Syntax};
use crate::directive::{Filter, Sampler};
use crate::egl::{EglContext, EglSurface};
use crate::keyboard::{self, Keys};
use crate::lua_images::Images;
use crate::multipass::{PassFile, ShaderFiles};
use crate::perf::{self, OutputStats};
use crate::pointer::{self, Drag};
use crate::render::Renderer;
use crate::script::{self, MouseState, Script};
use crate::shader::{self, Bindings, Channels, GpuTimer, PassKind, ShaderImages, ShaderProgram};
use crate::source::Source;
use crate::stderr::elog;
use crate::texture::{self, Texture};

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
    elog!("kagegami: {output}: {e}");
}

/// 描画の準備ができていなければ作る。作れなければ続けられないので終了する。
/// `App` のほかのフィールドを借りたまま呼べるように、`App` のメソッドにしない。
fn ensure_renderer<'a>(renderer: &'a mut Option<Renderer>, egl: &EglContext) -> &'a mut Renderer {
    if renderer.is_none() {
        match Renderer::new(egl) {
            Ok(r) => *renderer = Some(r),
            Err(e) => {
                elog!("kagegami: {e}");
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
    /// キーボードの入力（#69）。キーのイベントは、キーボードのフォーカスを持っている出力にだけ渡す（マウスと同じ）。
    keys: Keys,
    /// `--perf` で出す負荷の集計。
    stats: OutputStats,
    /// `--perf` のときだけ使う、Lua の壁紙の GPU 時間を測るタイマー。
    /// 描画するときに作る（EGL surface より先に Lua を読み込むことがあるため）。
    lua_timer: LuaTimer,
}

/// Lua の壁紙の GPU 時間を測るタイマーの状態。
enum LuaTimer {
    /// まだ作っていない。
    Unchecked,
    /// 拡張機能がなく、作れなかった。GPU 時間は出さない。
    Unavailable,
    Ready(GpuTimer),
}

/// 壁紙の中身。種類は起動時のファイルの拡張子で決まり、読み直しても変わらない。
enum Content {
    Lua(Script),
    /// パスごとのプログラムを持つので大きい。
    Shader(Box<ShaderState>),
}

/// 1 つのパスのプログラムと、チャンネルに渡すバッファー。
struct Pass {
    program: ShaderProgram,
    /// チャンネルごとの (バッファーの添字, 読むときの設定)。画像のチャンネルは None。
    buffer_refs: [Option<(usize, Sampler)>; 4],
    /// キーボードの入力を渡すチャンネル（#69）。
    keyboard: [bool; 4],
    /// チャンネルごとの、Cubemap A を読むときの filter（#72）。Cubemap A を渡さないチャンネルは None。
    cubemap: [Option<Filter>; 4],
}

impl Pass {
    fn compile(
        gl: &glow::Context,
        file: &PassFile,
        common: Option<&str>,
        kind: PassKind,
    ) -> Result<Self, String> {
        let dims = file.channel_dims();
        let program =
            ShaderProgram::new(gl, common, &file.src, kind, dims).map_err(|e| match kind {
                // 1 つのファイルだけの壁紙は、今までと同じメッセージにする。
                PassKind::Image if common.is_none() => e,
                _ => format!("{}: {e}", file.path.display()),
            })?;
        Ok(Self {
            program,
            buffer_refs: file.buffer_refs().map(|r| r.map(|r| (r.buffer, r.sampler))),
            keyboard: file.keyboard_channels(),
            cubemap: file.cubemap_refs().map(|r| r.map(|r| r.sampler.filter)),
        })
    }

    fn bindings(
        &self,
        channels: &Channels,
        buffers: Option<&BufferSet>,
        cube: Option<&CubemapPass>,
        keyboard: Option<&Texture>,
    ) -> Bindings {
        let mut b = Bindings::images(channels);
        if let Some(t) = keyboard {
            for (i, _) in self.keyboard.iter().enumerate().filter(|(_, k)| **k) {
                b.set_keyboard(i, t);
            }
        }
        if let Some(set) = buffers {
            for (i, r) in self.buffer_refs.iter().enumerate() {
                if let Some((k, sampler)) = r
                    && let Some(tex) = set.texture(*k)
                {
                    b.set_buffer(i, tex, set.sampler(*sampler), set.size());
                }
            }
        }
        if let Some(cube) = cube {
            for (i, f) in self.cubemap.iter().enumerate() {
                if let Some(f) = f {
                    b.set_cubemap(i, cube.texture(), cube.sampler(*f), cube.size());
                }
            }
        }
        b
    }
}

/// Image と Buffer A〜D と Cubemap A のパス。
struct Passes {
    image: Pass,
    buffers: [Option<Pass>; 4],
    cubemap: Option<Pass>,
}

impl Passes {
    /// すべてのパスをコンパイルする。どれかが失敗したら、作ったものを消してエラーを返す。
    fn compile(gl: &glow::Context, files: &ShaderFiles) -> Result<Self, String> {
        let common = files.common.as_ref().map(|(_, src)| src.as_str());
        let mut buffers: [Option<Pass>; 4] = Default::default();
        let mut result = Ok(());
        for (i, f) in files.buffers.iter().enumerate() {
            let Some(f) = f else { continue };
            match Pass::compile(gl, f, common, PassKind::Buffer) {
                Ok(p) => buffers[i] = Some(p),
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        let mut cubemap = None;
        if let (Ok(()), Some(f)) = (&result, &files.cubemap) {
            match Pass::compile(gl, f, common, PassKind::Cubemap) {
                Ok(p) => cubemap = Some(p),
                Err(e) => result = Err(e),
            }
        }
        let image = result.and_then(|()| Pass::compile(gl, &files.image, common, PassKind::Image));
        match image {
            Ok(image) => Ok(Self {
                image,
                buffers,
                cubemap,
            }),
            Err(e) => {
                for p in buffers.into_iter().chain([cubemap]).flatten() {
                    p.program.delete(gl);
                }
                Err(e)
            }
        }
    }

    fn delete(self, gl: &glow::Context) {
        self.image.program.delete(gl);
        for p in self.buffers.into_iter().chain([self.cubemap]).flatten() {
            p.program.delete(gl);
        }
    }
}

/// コンパイル済みのシェーダーと、`iTime`・`iFrame` の基準。
struct ShaderState {
    passes: Passes,
    /// すべてのパスの `iChannel0`〜`3` の画像。読み直すまで、すべての出力で同じものを使う。
    images: Rc<ShaderImages>,
    /// Buffer A〜D のテクスチャー（この出力のもの）。バッファーを使わない壁紙では None。
    buffers: Option<BufferSet>,
    /// Cubemap A のテクスチャー（この出力のもの、#72）。Cubemap A を使わない壁紙では None。
    cubemap: Option<CubemapPass>,
    /// Image を描く framebuffer。画面に描くときは None（テストで、画面の外に描くときだけ使う）。
    image_target: Option<glow::Framebuffer>,
    /// GPU 時間を測るタイマー。拡張機能がなければ None（見張らずに描く）。
    timer: Option<GpuTimer>,
    /// キーボードの入力のテクスチャー（この出力のもの）。キーボードを使わない壁紙では None。
    keyboard: Option<Texture>,
    started: Instant,
    frame: i32,
}

impl ShaderState {
    /// プログラムとバッファーを消し、タイマーを返す。画像は使う出力がなくなったら `AssetCache::collect` で消える。
    fn delete(self, gl: &glow::Context) -> Option<GpuTimer> {
        self.passes.delete(gl);
        if let Some(set) = self.buffers {
            set.delete(gl);
        }
        if let Some(c) = self.cubemap {
            c.delete(gl);
        }
        if let Some(t) = self.keyboard {
            texture::delete(gl, t);
        }
        self.timer
    }

    /// 1 フレーム描く。前のフレームまでに測れた GPU 時間が上限を超えていたら、描かずにエラーを返す。
    fn draw(
        &mut self,
        gl: &glow::Context,
        resolution: (u32, u32),
        mouse: [f32; 4],
        now: Instant,
        dt: f64,
        stats: &mut OutputStats,
    ) -> Result<(), String> {
        let times = self.timer.as_mut().map(|t| t.poll(gl)).unwrap_or_default();
        stats.gpu(times.as_slice());
        let slow = times.longest().filter(|d| *d > shader::FRAME_GPU_LIMIT);
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
        // 出力のサイズや --scale が変わったら、中身を写してサイズを合わせる（#57）。
        if let Some(set) = &mut self.buffers {
            set.resize(gl, resolution)?;
        }
        if let Some(t) = self.timer.as_mut() {
            t.begin(gl);
        }
        // A → B → C → D → Cubemap A → Image の順に描く。前に描いたパスは今のフレーム、
        // 自分と後ろのパスは前のフレームの結果を読む（Shadertoy と同じ。Cubemap A は #72 で確かめた）。
        for (i, pass) in self.passes.buffers.iter().enumerate() {
            let (Some(pass), Some(set), Some(channels)) =
                (pass, self.buffers.as_mut(), &self.images.buffers[i])
            else {
                continue;
            };
            let b = pass.bindings(
                channels,
                Some(set),
                self.cubemap.as_ref(),
                self.keyboard.as_ref(),
            );
            set.begin(gl, i);
            pass.program.draw(gl, &u, &b);
            set.end(gl, i);
        }
        if let (Some(pass), Some(cube), Some(channels)) = (
            &self.passes.cubemap,
            self.cubemap.as_mut(),
            &self.images.cubemap,
        ) {
            let b = pass.bindings(
                channels,
                self.buffers.as_ref(),
                Some(cube),
                self.keyboard.as_ref(),
            );
            pass.program.draw_cubemap(gl, &u, &b, cube);
        }
        // バッファーと Cubemap A は、描き終えると framebuffer を 0 に戻す。
        if let Some(t) = self.image_target {
            unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t)) };
        }
        let b = self.passes.image.bindings(
            &self.images.image,
            self.buffers.as_ref(),
            self.cubemap.as_ref(),
            self.keyboard.as_ref(),
        );
        self.passes.image.program.draw(gl, &u, &b);
        if let Some(t) = self.timer.as_mut() {
            t.end(gl);
        }
        self.frame = self.frame.wrapping_add(1);
        Ok(())
    }
}

/// Lua の `update` に渡す `dt` の上限（秒）。
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
            // キーボードの入力は、出力ごとに描く直前に送る（押したフレームの印が出力ごとに違うため）。
            if let Some(t) = &sh.keyboard {
                texture::update_keyboard(renderer.gl(), t, &w.keys.texture());
            }
            sh.draw(renderer.gl(), w.buffer_size, mouse, now, dt, &mut w.stats)
        }
        Some(Content::Lua(script)) if !w.failed => {
            let timer = match &mut w.lua_timer {
                LuaTimer::Ready(t) => Some(t),
                _ => None,
            };
            let t0 = Instant::now();
            let dl = script.frame(dt.min(MAX_LUA_DT))?;
            w.stats.lua(t0.elapsed(), dl.quads());
            match timer {
                Some(t) => {
                    w.stats.gpu(t.poll(renderer.gl()).as_slice());
                    t.begin(renderer.gl());
                    let r = renderer.submit(&dl, script.images());
                    t.end(renderer.gl());
                    r
                }
                None => renderer.submit(&dl, script.images()),
            }
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
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    pointer: Option<wl_pointer::WlPointer>,
    /// キーボードを使う壁紙の間だけ取る（使わない壁紙で keymap を読んで持たないため）。
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// キーボードを持つ seat。
    keyboard_seat: Option<wl_seat::WlSeat>,
    /// キーボードのフォーカスを持っている壁紙の surface。
    keyboard_focus: Option<wl_surface::WlSurface>,
    /// キーの番号から記号を引く、キーボードの配列。
    layout: keyboard::Layout,
    /// 壁紙の layer surface を `KeyboardInteractivity::OnDemand` にしているか（#69）。
    keyboard_on: bool,
    /// wlr-layer-shell が古く `on_demand` を使えないことを一度警告したか。
    keyboard_warned: bool,
    compositor: CompositorState,
    layer_shell: LayerShell,
    viewporter: Option<WpViewporter>,
    egl: EglContext,
    renderer: Option<Renderer>,
    /// タイマーの拡張機能がないことを一度警告したか。
    timer_warned: bool,
    /// バッファーを線形フィルターで読めないことを一度警告したか。
    float_linear_warned: bool,
    /// 壁紙の画像（シェーダーと Lua）。読み直すたびに 1 回だけ作り、すべての出力で使う。
    channels: AssetCache<ShaderImages>,
    lua_images: AssetCache<Images>,
    /// 見張るファイル（壁紙のファイルと、指定された画像）。変わったら `watch_changed` を立てる。
    watch: Vec<PathBuf>,
    watch_changed: bool,
    /// 壁紙のファイルの在りか（ファイル・ディレクトリ・zip ファイル）。
    source: Source,
    args: Args,
    frame_period: Duration,
    wallpapers: Vec<Wallpaper>,
    /// `--perf` のときだけ Some。
    perf: Option<PerfState>,
    #[cfg(feature = "gpu-bench")]
    bench: Option<crate::bench::Bench>,
}

/// 画像のテクスチャーなど、消すのに GL のコンテキストが要るもの。
trait GlAssets {
    fn delete(self, gl: &glow::Context);
}

impl GlAssets for ShaderImages {
    fn delete(self, gl: &glow::Context) {
        ShaderImages::delete(self, gl)
    }
}

impl GlAssets for Images {
    fn delete(self, gl: &glow::Context) {
        Images::delete(self, gl)
    }
}

/// 壁紙の画像の、読み直し 1 回分の結果。
struct AssetCache<T> {
    /// 次に読み込むときに作り直す（読み直しを始めたとき）。
    stale: bool,
    /// 作ったときの壁紙のファイルの中身と、その結果（エラーも、同じ読み直しの間は使い回す）。
    current: Option<(String, Result<Rc<T>, String>)>,
    /// 新しいものに置き換えたが、まだどこかの出力が使っているもの。使われなくなったら消す。
    retired: Vec<Rc<T>>,
}

impl<T> Default for AssetCache<T> {
    fn default() -> Self {
        Self {
            stale: false,
            current: None,
            retired: Vec::new(),
        }
    }
}

impl<T: GlAssets> AssetCache<T> {
    /// 壁紙のファイルの中身が `src` のときの画像。この読み直しでまだ作っていなければ `load` で作る。
    fn get(
        &mut self,
        src: &str,
        load: impl FnOnce() -> Result<T, String>,
    ) -> Result<Rc<T>, String> {
        let reuse = !self.stale && self.current.as_ref().is_some_and(|(s, _)| s == src);
        if !reuse {
            let loaded = load().map(Rc::new);
            if let Some((_, Ok(old))) = self.current.replace((src.to_string(), loaded)) {
                self.retired.push(old);
            }
            self.stale = false;
        }
        match &self.current {
            Some((_, r)) => r.clone(),
            None => unreachable!(),
        }
    }

    /// 使われなくなった画像のテクスチャーを消す。コンテキストが current のときに呼ぶ。
    fn collect(&mut self, gl: &glow::Context) {
        let mut keep = Vec::new();
        for a in self.retired.drain(..) {
            match Rc::try_unwrap(a) {
                Ok(a) => a.delete(gl),
                Err(a) => keep.push(a),
            }
        }
        self.retired = keep;
    }
}

/// `--perf` の表示の状態。
struct PerfState {
    /// 標準エラー出力が端末か。端末なら最後の行を書き換え、そうでなければ 1 行ずつ出す。
    tty: bool,
    /// 前に表示した時刻。
    last: Instant,
    meter: perf::ProcessMeter,
    gpu_meter: crate::gpu_usage::GpuUsageMeter,
}

const PERF_LIMITS: perf::Limits = perf::Limits {
    lua: script::FRAME_BUDGET,
    quads: script::MAX_FRAME_QUADS,
    shader_gpu: shader::FRAME_GPU_LIMIT,
};

impl App {
    /// Wayland の global を bind し、EGL を初期化する。壁紙は出力が見つかったときに作る。
    pub fn new(
        conn: &Connection,
        globals: &GlobalList,
        qh: &QueueHandle<Self>,
        args: Args,
        source: Source,
    ) -> Result<Self, String> {
        let compositor =
            CompositorState::bind(globals, qh).map_err(|e| format!("wl_compositor: {e}"))?;
        let layer_shell =
            LayerShell::bind(globals, qh).map_err(|e| format!("wlr-layer-shell: {e}"))?;
        // 縮小・拡大した描画を出力いっぱいに表示するには wp_viewporter が要る。
        let viewporter = if args.render_scale != 1.0 {
            Some(
                globals
                    .bind(qh, 1..=1, ())
                    .map_err(|e| format!("--scale requires wp_viewporter: {e}"))?,
            )
        } else {
            None
        };
        let egl = EglContext::new(conn.backend().display_ptr().cast())?;
        Ok(Self {
            registry_state: RegistryState::new(globals),
            output_state: OutputState::new(globals, qh),
            seat_state: SeatState::new(globals, qh),
            pointer: None,
            keyboard: None,
            keyboard_seat: None,
            keyboard_focus: None,
            layout: keyboard::Layout::default(),
            keyboard_on: false,
            keyboard_warned: false,
            compositor,
            layer_shell,
            viewporter,
            egl,
            renderer: None,
            timer_warned: false,
            float_linear_warned: false,
            channels: AssetCache::default(),
            lua_images: AssetCache::default(),
            watch: vec![source.watched_entry()],
            watch_changed: false,
            source,
            frame_period: Duration::from_secs_f64(1.0 / args.fps),
            perf: args.perf.then(|| PerfState {
                tty: std::io::IsTerminal::is_terminal(&std::io::stderr()),
                last: Instant::now(),
                meter: perf::ProcessMeter::new(),
                gpu_meter: crate::gpu_usage::GpuUsageMeter::new(),
            }),
            args,
            wallpapers: Vec::new(),
            #[cfg(feature = "gpu-bench")]
            bench: crate::bench::Bench::from_env(),
        })
    }

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
        // キーボードを使う壁紙のときだけ on_demand にする（#69）。使えるかは sync_keyboard で確かめてある。
        layer.set_keyboard_interactivity(if self.keyboard_on {
            KeyboardInteractivity::OnDemand
        } else {
            KeyboardInteractivity::None
        });
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
        elog!("kagegami: attached to output {name}");
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
            keys: Keys::default(),
            stats: OutputStats::default(),
            lua_timer: LuaTimer::Unchecked,
        });
    }

    fn is_shader(&self) -> bool {
        self.source.is_shader()
    }

    /// シェーダーのファイル（とバッファー・Common のファイル）を読んでコンパイルし、i 番目の出力の壁紙にする。
    /// 失敗したら、それまでのシェーダーをそのまま残してエラーを返す（Lua の読み直しと同じ）。
    fn load_shader(&mut self, i: usize) -> Result<(), String> {
        let script = self.source.entry();
        let mut watch = vec![script.to_path_buf()];
        let files = ShaderFiles::read(&self.source, script, &mut watch);
        self.set_watch_paths(watch);
        let files = files?;
        let es = self.wallpapers[i]
            .egl_surface
            .as_ref()
            .ok_or("no EGL surface")?;
        self.egl.make_current(es)?;
        let gl = ensure_renderer(&mut self.renderer, &self.egl).gl();
        let float_linear = if files.has_buffers() {
            let float_linear = buffers::check_support(gl)?;
            if !float_linear && files.filters_buffers() && !self.float_linear_warned {
                self.float_linear_warned = true;
                elog!(
                    "kagegami: warning: GL_OES_texture_float_linear is not available; \
                     buffers are read with filter=nearest"
                );
            }
            float_linear
        } else {
            false
        };
        if files.cubemap.is_some() {
            cubemap_pass::check_support(gl)?;
        }
        // コンパイルできないシェーダーのために画像を読まないように、先にコンパイルする。
        let passes = Passes::compile(gl, &files)?;
        let images = self.channels.get(&files.cache_key(), || {
            ShaderImages::load(gl, &self.source, &files)
        });
        // 読み直しに失敗し続けても、どの出力も使っていない画像が溜まらないようにする。
        self.channels.collect(gl);
        let images = match images {
            Ok(im) => im,
            Err(e) => {
                passes.delete(gl);
                return Err(e);
            }
        };
        // 読み直したときは、バッファーを 0 から作り直す（iFrame も 0 に戻る）。
        let buffers = if files.has_buffers() {
            let declared = files.buffers.each_ref().map(Option::is_some);
            let size = self.wallpapers[i].buffer_size;
            match BufferSet::new(gl, size, declared, files.mipmapped_buffers(), float_linear) {
                Ok(set) => Some(set),
                Err(e) => {
                    passes.delete(gl);
                    return Err(e);
                }
            }
        } else {
            None
        };
        // 読み直したときは、Cubemap A も 0 から作り直す（バッファーと同じ）。
        let cubemap = if files.cubemap.is_some() {
            match CubemapPass::new(gl, cubemap_pass::FACE_SIZE, files.mipmapped_cubemap()) {
                Ok(c) => Some(c),
                Err(e) => {
                    passes.delete(gl);
                    if let Some(set) = buffers {
                        set.delete(gl);
                    }
                    return Err(e);
                }
            }
        } else {
            None
        };
        let keyboard = if files.uses_keyboard() {
            match texture::keyboard(gl) {
                Ok(t) => Some(t),
                Err(e) => {
                    passes.delete(gl);
                    if let Some(set) = buffers {
                        set.delete(gl);
                    }
                    if let Some(c) = cubemap {
                        c.delete(gl);
                    }
                    return Err(e);
                }
            }
        } else {
            None
        };
        let old = match self.wallpapers[i].content.take() {
            Some(Content::Shader(old)) => Some(old),
            _ => None,
        };
        let timer = match old {
            Some(old) => {
                let timer = old.delete(gl);
                self.channels.collect(gl);
                timer.map(|mut t| {
                    // 前のシェーダーの結果で、直したシェーダーを止めないようにする。
                    t.reset();
                    t
                })
            }
            None => {
                let t = GpuTimer::new(gl, |n| self.egl.proc_address(n));
                if t.is_none() && !self.timer_warned {
                    self.timer_warned = true;
                    elog!(
                        "kagegami: warning: GL_EXT_disjoint_timer_query is not available; \
                         shaders that take too long on the GPU will not be stopped"
                    );
                }
                t
            }
        };
        self.wallpapers[i].content = Some(Content::Shader(Box::new(ShaderState {
            passes,
            images,
            buffers,
            cubemap,
            image_target: None,
            timer,
            keyboard,
            started: Instant::now(),
            frame: 0,
        })));
        Ok(())
    }

    /// 壁紙のファイルを読み、i 番目の出力の壁紙にする。`seed` は Lua の乱数の種。
    /// 失敗したら、それまでの壁紙をそのまま残してエラーを返す。
    fn load_content(&mut self, i: usize, seed: u64) -> Result<(), String> {
        if self.is_shader() {
            return self.load_shader(i);
        }
        let path = &self.source.entry().to_path_buf();
        let src = self.source.read_to_string(path)?;
        let specs =
            directive::parse(&src, Syntax::Lua).map_err(|e| format!("{}:{e}", path.display()))?;
        self.set_watch(&specs);
        // テクスチャーにするのは最初に描くとき。使われなくなった画像は render で消す。
        let source = &self.source;
        let images = self
            .lua_images
            .get(&src, || Images::load(source, &specs, path))?;
        let w = &mut self.wallpapers[i];
        let script = Script::with_images(
            &src,
            &path.display().to_string(),
            &w.name,
            seed,
            w.output_size,
            images,
        )?;
        w.content = Some(Content::Lua(script));
        Ok(())
    }

    /// 見張るファイルを、壁紙のファイルと指定された画像にする。
    /// 画像が読めなくても見張る（直して保存したら読み直すため）。
    fn set_watch(&mut self, specs: &[directive::ImageSpec]) {
        let script = self.source.entry();
        let mut watch = vec![script.to_path_buf()];
        watch.extend(specs.iter().map(|s| s.resolve(script)));
        self.set_watch_paths(watch);
    }

    fn set_watch_paths(&mut self, watch: Vec<PathBuf>) {
        let watch = self.source.watch_paths(watch);
        if watch != self.watch {
            self.watch = watch;
            self.watch_changed = true;
        }
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
        elog!("kagegami: reloading {}", self.args.script.display());
        self.source.reload();
        self.channels.stale = true;
        self.lua_images.stale = true;
        for i in 0..self.wallpapers.len() {
            if !self.can_load(i) {
                continue;
            }
            // 読み直すたびに新しい種を使う（#26）。
            let seed = seed_for(&self.wallpapers[i].seed);
            match self.load_content(i, seed) {
                Ok(()) => {
                    let w = &mut self.wallpapers[i];
                    // 直したあとの負荷だけを見られるように、最大値も消す。
                    w.stats.reset();
                    if let LuaTimer::Ready(t) = &mut w.lua_timer {
                        t.reset();
                    }
                    w.failed = false;
                    w.last_frame = None;
                    w.next_due = Instant::now();
                }
                Err(e) => report(&self.wallpapers[i].name, e),
            }
        }
        // keyboard_interactivity は、このあとの render_due で合わせる。
    }

    /// 壁紙がキーボードを使うか。シェーダーは `@iChannelN keyboard` の指定があるとき、Lua は
    /// `keyboard.down` を一度でも呼んだとき（#69）。
    fn wants_keyboard(&self) -> bool {
        self.wallpapers.iter().any(|w| match &w.content {
            Some(Content::Shader(sh)) => sh.keyboard.is_some(),
            Some(Content::Lua(script)) => script.uses_keyboard(),
            None => false,
        })
    }

    /// layer surface の keyboard_interactivity を、壁紙がキーボードを使うかに合わせる。
    /// キーボードを使わない壁紙では None のままにし、ほかのアプリへの入力を邪魔しない。
    fn sync_keyboard(&mut self, qh: &QueueHandle<Self>) {
        let want = self.wants_keyboard();
        if want == self.keyboard_on {
            return;
        }
        // on_demand は wlr-layer-shell のバージョン 4 から。古いコンポジターに送ると protocol error になる。
        let supported = self.wallpapers.iter().all(|w| match w.layer.kind() {
            SurfaceKind::Wlr(s) => s.version() >= 4,
            _ => false,
        });
        if want && !supported {
            if !self.keyboard_warned {
                self.keyboard_warned = true;
                elog!(
                    "kagegami: warning: the compositor does not support keyboard_interactivity \
                     on_demand (wlr-layer-shell v4); keyboard input is not available"
                );
            }
            return;
        }
        self.keyboard_on = want;
        let value = if want {
            KeyboardInteractivity::OnDemand
        } else {
            KeyboardInteractivity::None
        };
        for w in &mut self.wallpapers {
            w.layer.set_keyboard_interactivity(value);
            w.layer.commit();
        }
        if want {
            self.acquire_keyboard(qh);
        } else {
            self.release_keyboard();
        }
    }

    fn acquire_keyboard(&mut self, qh: &QueueHandle<Self>) {
        if self.keyboard.is_some() {
            return;
        }
        let Some(seat) = &self.keyboard_seat else {
            return;
        };
        match self.seat_state.get_keyboard(qh, seat, None) {
            Ok(k) => self.keyboard = Some(k),
            Err(e) => elog!("kagegami: keyboard: {e}"),
        }
    }

    fn release_keyboard(&mut self) {
        if let Some(k) = self.keyboard.take() {
            k.release();
        }
        self.keyboard_focus = None;
        self.layout = keyboard::Layout::default();
        for w in &mut self.wallpapers {
            w.keys.release_all();
        }
    }

    /// 見張るファイルが変わっていれば、その一覧を返す。
    pub fn take_watch_paths(&mut self) -> Option<Vec<PathBuf>> {
        std::mem::take(&mut self.watch_changed).then(|| self.watch.clone())
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

    /// 次に `--perf` の表示を更新する時刻までの待ち時間。`--perf` でなければ None。
    pub fn perf_timeout(&self, now: Instant) -> Option<Duration> {
        let p = self.perf.as_ref()?;
        Some((p.last + perf::INTERVAL).saturating_duration_since(now))
    }

    /// 前の表示から `perf::INTERVAL` 経っていれば、負荷の表示を更新する。
    pub fn perf_tick(&mut self, now: Instant) {
        let Some(p) = self.perf.as_mut() else {
            return;
        };
        if now < p.last + perf::INTERVAL {
            return;
        }
        let elapsed = now.duration_since(p.last);
        p.last = now;
        let shader_file = self.source.is_shader();
        let mut lua_bytes = None;
        let rows: Vec<_> = self
            .wallpapers
            .iter_mut()
            .map(|w| {
                let kind = match &w.content {
                    Some(Content::Shader(_)) => perf::Kind::Shader,
                    Some(Content::Lua(script)) => {
                        *lua_bytes.get_or_insert(0) += script.used_memory();
                        perf::Kind::Lua
                    }
                    None if shader_file => perf::Kind::Shader,
                    None => perf::Kind::Lua,
                };
                perf::take_row(&w.name, kind, w.failed, &mut w.stats, elapsed)
            })
            .collect();
        let (cpu_percent, rss_bytes) = p.meter.sample(now);
        let process = perf::ProcessRow {
            cpu_percent,
            gpu_engines: p.gpu_meter.sample(now),
            rss_bytes,
            lua_bytes,
        };
        if p.tty {
            crate::stderr::redraw_block(&perf::format_tty(
                &rows,
                &process,
                self.args.fps,
                &PERF_LIMITS,
            ));
        } else {
            elog!(
                "{}",
                perf::format_logfmt(&rows, &process, self.args.fps, &PERF_LIMITS)
            );
        }
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
        // Lua の壁紙は、keyboard.down を呼んだフレームのあとで on_demand にする。
        self.sync_keyboard(qh);
    }

    /// i 番目の出力の壁紙を 1 フレーム描いて提示する。
    fn render(&mut self, i: usize, qh: &QueueHandle<Self>, now: Instant) {
        let w = &mut self.wallpapers[i];
        if let Some(Content::Lua(script)) = &w.content {
            script.set_keys(w.keys.names_down());
            if let Err(e) = script.set_mouse(&w.mouse) {
                report(&w.name, e);
            }
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
        // 読み直して使われなくなった Lua の壁紙の画像を消す（読み込むときはコンテキストが無いことがある）。
        self.lua_images.collect(renderer.gl());
        if self.perf.is_some()
            && matches!(w.content, Some(Content::Lua(_)))
            && matches!(w.lua_timer, LuaTimer::Unchecked)
        {
            w.lua_timer = match GpuTimer::new(renderer.gl(), |n| self.egl.proc_address(n)) {
                Some(t) => LuaTimer::Ready(t),
                None => LuaTimer::Unavailable,
            };
        }
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
        w.stats.frame();
        w.failed |= failed;
        w.drag.frame_rendered();
        w.keys.frame_rendered();
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
            elog!("kagegami: output {} removed", self.wallpapers[i].name);
            self.remove_wallpaper(i);
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(i) = self.wallpapers.iter().position(|w| &w.layer == layer) {
            self.remove_wallpaper(i);
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
    /// i 番目の出力の壁紙を消す。GL のもの（シェーダーのプログラム・バッファー・タイマー）は、
    /// コンテキストを共有しているので、ほかの出力が残っていても消えない。ここで消す（#57）。
    fn remove_wallpaper(&mut self, i: usize) {
        let mut w = self.wallpapers.remove(i);
        let Some(s) = w.egl_surface.take() else {
            return;
        };
        if let Some(r) = &self.renderer
            && self.egl.make_current(&s).is_ok()
        {
            let gl = r.gl();
            if let Some(Content::Shader(sh)) = w.content.take()
                && let Some(t) = sh.delete(gl)
            {
                t.delete(gl);
            }
            if let LuaTimer::Ready(t) = std::mem::replace(&mut w.lua_timer, LuaTimer::Unavailable) {
                t.delete(gl);
            }
            self.channels.collect(gl);
        }
        // Lua の画像は、使う出力がなくなったら次の描画で消える。
        drop(w);
        self.egl.destroy_surface(s);
    }

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
            && let Err(e) = script.set_screen(w.output_size)
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
                Err(e) => elog!("kagegami: pointer: {e}"),
            }
        }
        if capability == Capability::Keyboard && self.keyboard_seat.is_none() {
            self.keyboard_seat = Some(seat);
            if self.keyboard_on {
                self.acquire_keyboard(qh);
            }
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && let Some(p) = self.pointer.take()
        {
            p.release();
        }
        if capability == Capability::Keyboard && self.keyboard_seat.as_ref() == Some(&seat) {
            self.keyboard_seat = None;
            self.release_keyboard();
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

impl App {
    fn release_all_keys(&mut self) {
        for w in &mut self.wallpapers {
            w.keys.release_all();
        }
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        raw: &[u32],
        keysyms: &[Keysym],
    ) {
        self.keyboard_focus = Some(surface.clone());
        let keys: Vec<_> = raw
            .iter()
            .zip(keysyms)
            .map(|(&r, &sym)| (r, self.layout.key(r, sym)))
            .collect();
        if let Some(w) = self.find(surface) {
            for (r, key) in keys {
                w.keys.hold(r, key);
            }
        }
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        // フォーカスが外れた出力のキーを離した扱いにする。ほかの出力は押していないので、全部に行ってよい。
        self.keyboard_focus = None;
        self.release_all_keys();
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        let key = self.layout.key(event.raw_code, event.keysym);
        let Some(surface) = self.keyboard_focus.clone() else {
            return;
        };
        if let Some(w) = self.find(&surface) {
            w.keys.press(event.raw_code, key);
        }
    }

    /// キーのリピートは、押したままと同じなので使わない。
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        // 押していない出力では何も変わらないので、全部の出力で離す。
        for w in &mut self.wallpapers {
            w.keys.release(event.raw_code);
        }
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: RawModifiers,
        layout: u32,
    ) {
        self.layout.set_group(layout);
    }

    fn update_keymap(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        keymap: smithay_client_toolkit::seat::keyboard::Keymap<'_>,
    ) {
        self.layout.set_keymap(keymap.as_string());
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

#[cfg(test)]
mod cubemap_tests {
    use super::*;

    /// 向きを確かめる方向。面ごとに 1 つ、中心から外れた非対称な位置（+X, -X, +Y, -Y, +Z, -Z の順）。
    const DIRS: [[f32; 3]; 6] = [
        [1.0, 0.3, -0.55],
        [-1.0, -0.4, 0.2],
        [0.35, 1.0, -0.6],
        [-0.45, -1.0, 0.25],
        [0.5, -0.3, 1.0],
        [-0.25, 0.55, -1.0],
    ];

    const CUBE: &str = "// @iChannel0 cubemapA filter=nearest
void mainCubemap(out vec4 c, in vec2 p, in vec3 ro, in vec3 rd) {
    if (rd.y > 0.99) {
        // +Y の面の中心: (iResolution.xy, 自分を読んだ iFrame の差 + rayOri の長さ, iFrame)
        float prev = texture(iChannel0, rd).w;
        c = vec4(iResolution.xy, float(iFrame) - prev + length(ro) * 100.0, float(iFrame));
    } else {
        c = vec4(rd, float(iFrame));
    }
}
";

    const BUFFER: &str = "// @iChannel0 cubemapA filter=nearest
void mainImage(out vec4 c, in vec2 p) {
    c = vec4(float(iFrame) - texture(iChannel0, vec3(0.0, 1.0, 0.0)).w);
}
";

    /// x = 0〜5: DIRS の方向で読んだ rayDir（* 0.5 + 0.5）。
    /// x = 6: (Image から読んだ iFrame の差, Buffer A から読んだ iFrame の差, Cubemap A が自分を読んだ差) / 255。
    /// x = 7: (Cubemap A の中の iResolution.xy, iChannelResolution[0].x) / 255。
    const IMAGE: &str = "// @bufferA a.glsl
// @cubemapA cube.glsl
// @iChannel0 cubemapA filter=nearest
// @iChannel1 bufferA filter=nearest
const vec3 DIRS[6] = vec3[6](vec3(1.0, 0.3, -0.55), vec3(-1.0, -0.4, 0.2), vec3(0.35, 1.0, -0.6),
                             vec3(-0.45, -1.0, 0.25), vec3(0.5, -0.3, 1.0), vec3(-0.25, 0.55, -1.0));
void mainImage(out vec4 c, in vec2 p) {
    int x = int(p.x);
    vec4 top = texture(iChannel0, vec3(0.0, 1.0, 0.0));
    if (x < 6) {
        c = vec4(texture(iChannel0, DIRS[x]).xyz * 0.5 + 0.5, 1.0);
    } else if (x == 6) {
        float buf = texelFetch(iChannel1, ivec2(0), 0).x;
        c = vec4(float(iFrame) - top.w, buf, top.z, 255.0) / 255.0;
    } else {
        c = vec4(top.xy, iChannelResolution[0].x, 255.0) / 255.0;
    }
}
";

    /// `dir` の Image・Buffer A・Cubemap A のファイルで、アプリと同じ経路（ファイルの読み取り → コンパイル →
    /// `ShaderState::draw`）で描き、`out_size` の Image の結果を読み出す。`frames` はフレームごとの出力のサイズ。
    fn draw_frames(
        gl: &glow::Context,
        dir: &std::path::Path,
        face: u32,
        frames: &[(u32, u32)],
        out_size: (u32, u32),
    ) -> Vec<u8> {
        let source = Source::open(&dir.join("main.glsl")).unwrap();
        let files = ShaderFiles::read(&source, source.entry(), &mut vec![]).unwrap();
        cubemap_pass::check_support(gl).unwrap();
        let float_linear = buffers::check_support(gl).unwrap();
        let passes = Passes::compile(gl, &files).unwrap();
        let images = Rc::new(ShaderImages::load(gl, &source, &files).unwrap());
        let declared = files.buffers.each_ref().map(Option::is_some);
        let buffers = BufferSet::new(
            gl,
            frames[0],
            declared,
            files.mipmapped_buffers(),
            float_linear,
        )
        .unwrap();
        let mut state = ShaderState {
            passes,
            images: images.clone(),
            buffers: Some(buffers),
            cubemap: Some(CubemapPass::new(gl, face, files.mipmapped_cubemap()).unwrap()),
            image_target: None,
            timer: None,
            keyboard: None,
            started: Instant::now(),
            frame: 0,
        };
        eprintln!("renderer: {}", unsafe {
            gl.get_parameter_string(glow::RENDERER)
        });
        let mut stats = OutputStats::default();
        let out = crate::gpu_test::render(gl, out_size, || {
            state.image_target =
                unsafe { gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING) };
            for size in frames {
                state
                    .draw(gl, *size, [0.0; 4], Instant::now(), 0.0, &mut stats)
                    .unwrap();
            }
        });
        assert_eq!(unsafe { gl.get_error() }, glow::NO_ERROR);
        state.delete(gl);
        Rc::try_unwrap(images).ok().unwrap().delete(gl);
        out
    }

    fn write_files(name: &str, image: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kagegami-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.glsl"), image).unwrap();
        std::fs::write(dir.join("a.glsl"), BUFFER).unwrap();
        std::fs::write(dir.join("cube.glsl"), CUBE).unwrap();
        dir
    }

    fn px(out: &[u8], x: usize) -> [u8; 3] {
        let o = x * 4;
        [out[o], out[o + 1], out[o + 2]]
    }

    /// 向き・描く順番・`iResolution`・`iChannelResolution` が、#72 で確かめた Shadertoy の値になる。
    /// 出力のサイズが変わっても、Cubemap A の中身は残る。
    #[test]
    fn cubemap_pass_matches_shadertoy() {
        let Some((_egl, gl)) = crate::gpu_test::context() else {
            eprintln!("skipped: no surfaceless EGL display");
            return;
        };
        let dir = write_files("cubepass", IMAGE);
        // 出力は正方形でなく、縦は面のサイズの半分より小さくする（iResolution と viewport の取り違えを
        // 見つけるため）。3 フレーム目でサイズを変える。
        let out = draw_frames(&gl, &dir, 16, &[(40, 4), (40, 4), (32, 4)], (40, 4));
        for (x, d) in DIRS.iter().enumerate() {
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let got = px(&out, x).map(|v| v as f32 / 255.0 * 2.0 - 1.0);
            for c in 0..3 {
                // 面の画素の中心の方向なので、半画素ぶん（16 の面で 1/16）と 8 ビットの丸めだけずれる。
                assert!(
                    (got[c] - d[c] / len).abs() < 0.1,
                    "face {x}: got {got:?}, want {:?}",
                    d.map(|v| v / len)
                );
            }
        }
        // Image から読むと今のフレーム（差 0）、Buffer A と Cubemap A 自身からは前のフレーム（差 1）。
        // rayOri は原点。サイズを変えたフレームでも自分の前のフレームを読める（作り直すと差が 2 になる）。
        assert_eq!(px(&out, 6), [0, 1, 1]);
        // Cubemap A の中の iResolution は出力のサイズ、読む側の iChannelResolution は面のサイズ。
        assert_eq!(px(&out, 7), [32, 4, 16]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 面のサイズ 1024 で `filter=mipmap` で読むと、描くたびにミップマップを作る。
    #[test]
    fn mipmapped_cubemap_pass_at_full_size() {
        let Some((_egl, gl)) = crate::gpu_test::context() else {
            eprintln!("skipped: no surfaceless EGL display");
            return;
        };
        // x = 0〜5: DIRS の方向の、いちばん粗い段（1×1）の値。面の rayDir の平均なので、面の向きを向く。
        let image = IMAGE
            .replace(
                "// @iChannel0 cubemapA filter=nearest",
                "// @iChannel0 cubemapA filter=mipmap",
            )
            .replace(
                "texture(iChannel0, DIRS[x]).xyz",
                "textureLod(iChannel0, DIRS[x], 10.0).xyz",
            );
        let dir = write_files("cubepass-mip", &image);
        let face = crate::cubemap_pass::FACE_SIZE;
        let out = draw_frames(&gl, &dir, face, &[(40, 4); 2], (40, 4));
        // (+X, -X, +Y, -Y, +Z, -Z) の面の向きの成分。+Y の面は、中心に iResolution などを書いていて
        // 平均が rayDir にならないので確かめない。
        for (x, (axis, sign)) in [
            (0, 1.0),
            (0, -1.0),
            (1, 1.0),
            (1, -1.0),
            (2, 1.0),
            (2, -1.0),
        ]
        .into_iter()
        .enumerate()
        .filter(|(x, _)| *x != 2)
        {
            let got = px(&out, x).map(|v| v as f32 / 255.0 * 2.0 - 1.0);
            assert!(got[axis] * sign > 0.5, "face {x}: {got:?}");
        }
        assert_eq!(
            px(&out, 7)[2],
            255,
            "iChannelResolution is 1024 (clamped to 255)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

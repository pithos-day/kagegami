//! 壁紙スクリプト（LuaJIT）の実行環境。
//!
//! 出力ごとに独立した Lua state を 1 つ持つ。スクリプトに渡すのは画面とマウスの情報だけで、
//! ファイルや OS には触れられないようにしている。

use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, Table, VmState};

use crate::render::{Batch, Blend, Vertex};

/// スクリプトの 1 回の呼び出しに許す時間。超えたらエラーにして止める。
const LOAD_BUDGET: Duration = Duration::from_secs(2);
const FRAME_BUDGET: Duration = Duration::from_millis(250);
/// 時間切れを確認する間隔（命令数）。
const HOOK_INTERVAL: u32 = 10_000;

/// スクリプトが 1 フレームで積んだ描画命令。
#[derive(Default)]
pub struct DrawList {
    pub clear: Option<[f32; 3]>,
    pub batch: Batch,
    /// (ブレンドモード, この区間が始まる四角形の番号)。
    pub segments: Vec<(Blend, usize)>,
    blend: Blend,
}

impl DrawList {
    fn reset(&mut self) {
        self.clear = None;
        self.batch.vertices.clear();
        self.batch.color = [255; 4];
        self.segments.clear();
        self.blend = Blend::Alpha;
        self.segments.push((Blend::Alpha, 0));
    }

    fn set_blend(&mut self, blend: Blend) {
        if blend == self.blend {
            return;
        }
        self.blend = blend;
        let at = self.batch.quads();
        match self.segments.last_mut() {
            Some(last) if last.1 == at => last.0 = blend,
            _ => self.segments.push((blend, at)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MouseState {
    pub x: f32,
    pub y: f32,
    pub inside: bool,
    pub left: bool,
    pub right: bool,
    pub middle: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum MouseEvent {
    Pressed(f32, f32, Button),
    Released(f32, f32, Button),
}

struct Deadline(Instant);

/// スクリプトを実行し始めた時刻（プロセス起動からのミリ秒 + 1）。実行していなければ 0。
static BUSY_SINCE: AtomicU64 = AtomicU64::new(0);
static EPOCH: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// スクリプトの実行中であることを watchdog に知らせる。
struct BusyGuard;

impl BusyGuard {
    fn new() -> Self {
        BUSY_SINCE.store(now_ms(), Ordering::Relaxed);
        BusyGuard
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY_SINCE.store(0, Ordering::Relaxed);
    }
}

/// watchdog が終了させたときの終了コード。再起動するかどうかの判断に使う。
pub const WATCHDOG_EXIT_CODE: i32 = 3;

/// スクリプトが `limit` 以上戻ってこなければプロセスを終了させる見張りスレッドを起動する。
///
/// JIT コンパイルされたループは hook では止まらないため、これが最後の歯止めになる。
/// Lua を外から安全に止める手段はないので、終了して再起動（systemd の Restart= など）に任せる。
pub fn spawn_watchdog(limit: Duration) {
    now_ms();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        let since = BUSY_SINCE.load(Ordering::Relaxed);
        if since != 0 && now_ms().saturating_sub(since) > limit.as_millis() as u64 {
            eprintln!("kagegami: wallpaper script did not return for {}s, exiting", limit.as_secs());
            // メインスレッドは Lua の中で止まっているので、後始末をせずに終了する。
            unsafe { libc::_exit(WATCHDOG_EXIT_CODE) };
        }
    });
}

pub struct Script {
    // lua を先に drop する（prelude が _buf を指しているため、宣言順を変えないこと）。
    lua: Lua,
    screen: Table,
    mouse: Table,
    #[allow(dead_code)]
    jit_enabled: bool,
    begin: Function,
    finish: Function,
    /// prelude が ffi で直接書き込む頂点バッファー。確保した後は動かさない。
    _buf: Box<[Vertex]>,
}

/// 壁紙スクリプトに見せないグローバル。ファイル読み込みや別環境への抜け道になるもの。
const REMOVED_GLOBALS: &[&str] = &[
    "dofile", "loadfile", "load", "loadstring", "require", "module",
    "getfenv", "setfenv", "collectgarbage", "gcinfo", "newproxy", "jit", "ffi",
];

/// prelude の頂点バッファーの容量（頂点数、4 の倍数）。満杯になるたびに DrawList へ移す。
const PRELUDE_CAP: usize = 4096;

/// 描画関数（color, line など）を prelude.lua で作り、グローバルに置く。
///
/// prelude は ffi で `buf` に直接頂点を書き、満杯になったときやフレームの終わりに
/// `take(n)` で DrawList へ移す。ffi は prelude の upvalue にしか残らない。
fn install_prelude(lua: &Lua, buf: *mut Vertex) -> mlua::Result<(Function, Function)> {
    let base = buf as usize;
    let take = lua.create_function(move |lua, n: i64| {
        if n < 0 || n as usize > PRELUDE_CAP || n % 4 != 0 {
            return Err(mlua::Error::runtime(format!("internal: bad vertex count {n}")));
        }
        // 安全性: buf は PRELUDE_CAP 個の Vertex を持つ Box で、Script が生きている間は動かない。
        let src = unsafe { std::slice::from_raw_parts(base as *const Vertex, n as usize) };
        lua.app_data_mut::<DrawList>().unwrap().batch.vertices.extend_from_slice(src);
        Ok(())
    })?;
    let clear = lua.create_function(|lua, (r, g, b): (f32, f32, f32)| {
        let mut dl = lua.app_data_mut::<DrawList>().unwrap();
        dl.batch.vertices.clear();
        let blend = dl.blend;
        dl.segments.clear();
        dl.segments.push((blend, 0));
        dl.clear = Some([r, g, b]);
        Ok(())
    })?;
    let blend = lua.create_function(|lua, mode: mlua::Value| {
        let blend = match mode.as_string().map(|s| s.as_bytes().to_vec()).as_deref() {
            Some(b"alpha") => Blend::Alpha,
            Some(b"add") => Blend::Add,
            _ => return Err(mlua::Error::runtime("blend: expected \"alpha\" or \"add\"")),
        };
        lua.app_data_mut::<DrawList>().unwrap().set_blend(blend);
        Ok(())
    })?;
    let g = lua.globals();
    let ffi: Table = g.get("ffi")?;
    let math: Table = g.get("math")?;
    let (api, host): (Table, Table) = lua
        .load(include_str!("prelude.lua"))
        .set_name("=kagegami")
        .set_environment(lua.create_table()?)
        .call((
            ffi,
            mlua::LightUserData(buf.cast()),
            PRELUDE_CAP,
            take,
            clear,
            blend,
            math.get::<Function>("sqrt")?,
            g.get::<Function>("error")?,
            g.get::<Function>("type")?,
        ))?;
    for name in ["color", "circle", "line", "rect", "clear", "blend"] {
        g.set(name, api.get::<Function>(name)?)?;
    }
    Ok((host.get("begin")?, host.get("finish")?))
}

impl Script {
    pub fn load(path: &Path, output: &str, seed: u64, width: u32, height: u32) -> Result<Self, String> {
        let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_source(&src, &path.display().to_string(), output, seed, width, height)
    }

    pub fn from_source(src: &str, chunk_name: &str, output: &str, seed: u64, width: u32, height: u32) -> Result<Self, String> {
        // LuaJIT は jit ライブラリを開いたときに JIT コンパイラーを有効にするので、JIT も読み込む。
        // スクリプトから jit.off() などを呼べないよう、読み込んだ後にグローバルから外す。
        // ffi は描画関数（prelude）の中だけで使い、読み込んだ後にグローバルから外す。
        // ffi を開くには mlua の safe モードを外す必要がある。
        let libs = StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::BIT | StdLib::JIT | StdLib::FFI;
        let lua = unsafe { Lua::unsafe_new_with(libs, LuaOptions::default()) };
        let _busy = BusyGuard::new();
        let buf = vec![Vertex::default(); PRELUDE_CAP].into_boxed_slice();
        let script = Self::setup(lua, buf, output, seed, width, height).map_err(|e| e.to_string())?;
        script.lua.set_app_data(Deadline(Instant::now() + LOAD_BUDGET));
        script
            .lua
            .load(src)
            .set_name(format!("@{chunk_name}"))
            .set_mode(mlua::chunk::ChunkMode::Text)
            .exec()
            .map_err(|e| e.to_string())?;
        script.call("load", ()).map_err(|e| e.to_string())?;
        Ok(script)
    }

    fn setup(lua: Lua, buf: Box<[Vertex]>, output: &str, seed: u64, width: u32, height: u32) -> mlua::Result<Self> {
        let buf_ptr = buf.as_ptr() as *mut Vertex;
        let g = lua.globals();
        let jit: Table = g.get("jit")?;
        let jit_enabled: bool = jit.get::<Function>("status")?.call(())?;
        if !jit_enabled {
            eprintln!("kagegami: warning: LuaJIT JIT compiler is disabled");
        }
        let string: Table = g.get("string")?;
        string.raw_set("dump", mlua::Nil)?;

        // 出力ごとに乱数を変える（スクリプトからは時刻を取れないため）。
        let math: Table = g.get("math")?;
        math.get::<Function>("randomseed")?.call::<()>((seed % (1 << 52)) as f64)?;

        // print は stderr に出力名付きで出す。
        let prefix = format!("[{output}]");
        g.set(
            "print",
            lua.create_function(move |_, args: mlua::Variadic<mlua::Value>| {
                let parts: Vec<String> = args.iter().map(|v| v.to_string().unwrap_or_else(|_| "?".into())).collect();
                eprintln!("{prefix} {}", parts.join("\t"));
                Ok(())
            })?,
        )?;

        lua.set_app_data(DrawList::default());
        lua.app_data_mut::<DrawList>().unwrap().reset();

        let (begin, finish) = install_prelude(&lua, buf_ptr)?;
        for name in REMOVED_GLOBALS {
            g.raw_set(*name, mlua::Nil)?;
        }

        let screen = lua.create_table()?;
        screen.set("w", width)?;
        screen.set("h", height)?;
        screen.set("output", output)?;
        g.set("screen", &screen)?;
        let mouse = lua.create_table()?;
        g.set("mouse", &mouse)?;

        // 無限ループ対策。JIT コンパイルされたコードでは呼ばれないことがあるので、
        // 最終的な歯止めは main 側の watchdog が担う。
        lua.set_hook(HookTriggers::new().every_nth_instruction(HOOK_INTERVAL), |lua, _| {
            match lua.app_data_ref::<Deadline>() {
                Some(d) if Instant::now() > d.0 => Err(mlua::Error::runtime("time limit exceeded")),
                _ => Ok(VmState::Continue),
            }
        })?;

        let s = Self { lua, screen, mouse, jit_enabled, begin, finish, _buf: buf };
        s.set_mouse(&MouseState::default()).map_err(mlua::Error::runtime)?;
        Ok(s)
    }

    fn call(&self, name: &str, args: impl mlua::IntoLuaMulti) -> mlua::Result<()> {
        match self.lua.globals().raw_get::<Option<Function>>(name)? {
            Some(f) => f.call(args),
            None => Ok(()),
        }
    }

    pub fn set_screen(&self, width: u32, height: u32) -> Result<(), String> {
        (|| -> mlua::Result<()> {
            self.screen.set("w", width)?;
            self.screen.set("h", height)
        })()
        .map_err(|e| e.to_string())
    }

    pub fn set_mouse(&self, m: &MouseState) -> Result<(), String> {
        (|| -> mlua::Result<()> {
            self.mouse.set("x", m.x)?;
            self.mouse.set("y", m.y)?;
            self.mouse.set("inside", m.inside)?;
            self.mouse.set("left", m.left)?;
            self.mouse.set("right", m.right)?;
            self.mouse.set("middle", m.middle)
        })()
        .map_err(|e| e.to_string())
    }

    /// マウスのボタン操作をコールバックに渡す。
    pub fn mouse_event(&self, ev: MouseEvent) -> Result<(), String> {
        let _busy = BusyGuard::new();
        self.lua.set_app_data(Deadline(Instant::now() + FRAME_BUDGET));
        match ev {
            MouseEvent::Pressed(x, y, b) => self.call("mousepressed", (x, y, b.name())),
            MouseEvent::Released(x, y, b) => self.call("mousereleased", (x, y, b.name())),
        }
        .map_err(|e| e.to_string())
    }

    /// update と draw を呼び、積まれた描画命令を返す。
    pub fn frame(&self, dt: f64) -> Result<mlua::AppDataRef<'_, DrawList>, String> {
        let _busy = BusyGuard::new();
        self.lua.set_app_data(Deadline(Instant::now() + FRAME_BUDGET));
        self.lua.app_data_mut::<DrawList>().unwrap().reset();
        self.begin.call::<()>(()).map_err(|e| e.to_string())?;
        self.call("update", dt).map_err(|e| e.to_string())?;
        self.call("draw", ()).map_err(|e| e.to_string())?;
        self.finish.call::<()>(()).map_err(|e| e.to_string())?;
        Ok(self.lua.app_data_ref::<DrawList>().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> Result<Script, String> {
        Script::from_source(src, "test", "TEST-1", 1, 800, 600)
    }

    #[test]
    fn sandbox_hides_dangerous_globals() {
        let s = run("").unwrap();
        let check = r#"
            local names = { "io", "os", "require", "package", "debug", "ffi", "jit",
                "dofile", "loadfile", "load", "loadstring", "getfenv", "setfenv",
                "collectgarbage", "module" }
            for _, n in ipairs(names) do
                assert(_G[n] == nil, n .. " should be nil")
            end
            assert(string.dump == nil, "string.dump should be nil")
        "#;
        s.lua.load(check).exec().unwrap();
    }

    #[test]
    fn jit_compiler_is_enabled() {
        assert!(run("").unwrap().jit_enabled);
    }

    #[test]
    fn host_functions_do_not_leak_environment() {
        // getfenv が無いので、ホスト関数経由で別の環境に辿り着けない。
        let r = run("local e = getfenv(line)");
        assert!(r.is_err());
    }

    #[test]
    fn draw_api_records_vertices_and_blend_segments() {
        let s = run(r#"
            function draw()
                clear(0.1, 0.2, 0.3)
                color(1, 0, 0, 0.5)
                circle(10, 10, 5)
                blend("add")
                line(0, 0, screen.w, screen.h, 2)
                rect(1, 2, 3, 4)
            end
        "#)
        .unwrap();
        let dl = s.frame(1.0 / 30.0).unwrap();
        assert_eq!(dl.clear, Some([0.1, 0.2, 0.3]));
        assert_eq!(dl.batch.quads(), 3);
        assert_eq!(dl.segments, vec![(Blend::Alpha, 0), (Blend::Add, 1)]);
        assert_eq!(dl.batch.vertices[0].color, [255, 0, 0, 128]);
    }

    #[test]
    fn callbacks_receive_dt_and_mouse() {
        let s = run(r#"
            got = {}
            function update(dt) got.dt = dt end
            function mousepressed(x, y, b) got.click = b .. ":" .. x .. "," .. y end
        "#)
        .unwrap();
        s.set_mouse(&MouseState { x: 3.0, y: 4.0, inside: true, left: true, ..Default::default() }).unwrap();
        s.mouse_event(MouseEvent::Pressed(3.0, 4.0, Button::Left)).unwrap();
        drop(s.frame(0.5).unwrap());
        s.lua.load(r#"
            assert(got.dt == 0.5)
            assert(got.click == "left:3,4")
            assert(mouse.inside == true and mouse.left == true and mouse.x == 3)
        "#).exec().unwrap();
    }

    /// hook で止まるのはインタープリターで動いている間だけ。JIT コンパイルされたループは
    /// hook では止まらない（計測で確認済み）ので、そちらは main 側の watchdog が止める。
    /// ここではクロージャ生成（LuaJIT が JIT できない命令）を入れて、インタープリターで回す。
    #[test]
    fn infinite_loop_in_interpreter_is_stopped() {
        let s = run("function update() local x = 0 while true do x = x + 1; local f = function() end end end").unwrap();
        let start = Instant::now();
        let err = s.frame(0.0).err().unwrap();
        assert!(err.contains("time limit exceeded"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn prelude_matches_rust_reference_geometry() {
        let s = run(r#"
            function draw()
                color(0.2, 0.4, 0.6, 0.8)
                line(10.5, 20, 300, 140.25, 3)
                circle(50, 60, 7.5)
                rect(1, 2, 30, 40)
                line(5, 5, 5, 5, 4)
            end
        "#)
        .unwrap();
        let dl = s.frame(0.0).unwrap();
        let mut reference = Batch { vertices: Vec::new(), color: crate::render::color_u8(0.2, 0.4, 0.6, 0.8) };
        reference.line(10.5, 20.0, 300.0, 140.25, 3.0);
        reference.circle(50.0, 60.0, 7.5);
        reference.rect(1.0, 2.0, 30.0, 40.0);
        reference.line(5.0, 5.0, 5.0, 5.0, 4.0);
        assert_eq!(dl.batch.vertices.len(), reference.vertices.len());
        for (a, b) in dl.batch.vertices.iter().zip(&reference.vertices) {
            let close = |x: [f32; 2], y: [f32; 2]| (x[0] - y[0]).abs() < 1e-3 && (x[1] - y[1]).abs() < 1e-3;
            assert!(close(a.pos, b.pos) && close(a.local, b.local) && close(a.shape, b.shape), "{a:?} != {b:?}");
            assert_eq!(a.color, b.color);
        }
    }

    #[test]
    fn ffi_is_unreachable_and_draw_functions_return_nothing() {
        let s = run("").unwrap();
        s.lua.load(r#"
            assert(ffi == nil and jit == nil)
            assert(select('#', line(0, 0, 1, 1)) == 0)
            assert(select('#', circle(0, 0, 1)) == 0)
            assert(select('#', rect(0, 0, 1, 1)) == 0)
            assert(select('#', color(1, 1, 1)) == 0)
            -- 描画関数は C 関数ではなく Lua 関数だが、upvalue を覗く手段がない
            assert(debug == nil and getfenv == nil and string.dump == nil)
        "#).exec().unwrap();
    }

    #[test]
    fn many_shapes_overflow_the_ffi_buffer_safely() {
        // バッファー（PRELUDE_CAP 頂点）を何度も溢れさせ、途中でブレンドも切り替える。
        let s = run(r#"
            function draw()
                for i = 1, 5000 do line(i, 0, i, 10, 1) end
                blend("add")
                for i = 1, 3000 do circle(i, 5, 2) end
            end
        "#)
        .unwrap();
        for _ in 0..3 {
            let dl = s.frame(0.0).unwrap();
            assert_eq!(dl.batch.quads(), 8000);
            assert_eq!(dl.segments, vec![(Blend::Alpha, 0), (Blend::Add, 5000)]);
        }
    }

    #[test]
    fn clear_discards_shapes_even_after_flushes() {
        let s = run(r#"
            function draw()
                for i = 1, 3000 do circle(i, 5, 2) end
                clear(0, 0, 0)
                circle(1, 1, 1)
            end
        "#)
        .unwrap();
        assert_eq!(s.frame(0.0).unwrap().batch.quads(), 1);
    }

    #[test]
    fn odd_values_are_handled() {
        let s = run(r#"
            function draw()
                color(0/0, 2, -1, 1e300)
                line(0, 0, 1e30, -1e30, 0/0)
                circle(0/0, 0, -5)
            end
        "#)
        .unwrap();
        let dl = s.frame(0.0).unwrap();
        assert_eq!(dl.batch.vertices[0].color, [0, 255, 0, 255]);
        assert_eq!(dl.batch.quads(), 2);
        drop(dl);
        assert!(run("function draw() line('a', 0, 1, 1) end").unwrap().frame(0.0).err().unwrap().contains("number expected"));
        assert!(run("function draw() blend('multiply') end").unwrap().frame(0.0).is_err());
    }

    #[test]
    fn errors_mid_draw_do_not_leak_into_next_frame() {
        let s = run(r#"
            n = 0
            function draw()
                n = n + 1
                circle(0, 0, 1)
                if n == 1 then error("boom") end
            end
        "#)
        .unwrap();
        assert!(s.frame(0.0).is_err());
        assert_eq!(s.frame(0.0).unwrap().batch.quads(), 1);
    }

    #[test]
    fn seeds_differ_per_output() {
        let a = Script::from_source("v = math.random()", "t", "A", 1, 1, 1).unwrap();
        let b = Script::from_source("v = math.random()", "t", "B", 2, 1, 1).unwrap();
        let va: f64 = a.lua.globals().get("v").unwrap();
        let vb: f64 = b.lua.globals().get("v").unwrap();
        assert_ne!(va, vb);
    }

    #[test]
    fn syntax_and_runtime_errors_are_reported() {
        assert!(run("function (").is_err());
        let s = run("function draw() nope() end").unwrap();
        assert!(s.frame(0.0).is_err());
    }
}

#[cfg(test)]
mod bench {
    use mlua::{HookTriggers, Lua, VmState};
    use std::time::Instant;

    const SRC: &str = "local s = 0 for i = 1, 3e7 do s = s + (i % 7) * 0.5 end return s";

    fn time(lua: &Lua) -> f64 {
        let t = Instant::now();
        let _: f64 = lua.load(SRC).eval().unwrap();
        t.elapsed().as_secs_f64()
    }

    #[test]
    #[ignore]
    fn hook_cost() {
        let plain = Lua::new();
        let hooked = Lua::new();
        hooked.set_hook(HookTriggers::new().every_nth_instruction(10_000), |_, _| Ok(VmState::Continue)).unwrap();
        let sandboxed = Lua::new_with(mlua::StdLib::MATH, mlua::LuaOptions::default()).unwrap();
        eprintln!("no jit lib: {:.3}s", time(&sandboxed));
        let with_jit = Lua::new_with(mlua::StdLib::MATH | mlua::StdLib::JIT, mlua::LuaOptions::default()).unwrap();
        eprintln!("jit lib  : {:.3}s", time(&with_jit));
        let jit_off = Lua::new();
        jit_off.load("jit.off()").exec().unwrap();
        let st: String = plain.load("local t = {jit.status()} local o = {} for i, v in ipairs(t) do o[#o+1] = tostring(v) end return jit.version .. ' ' .. table.concat(o, ' ')").eval().unwrap();
        eprintln!("status : {st}");
        eprintln!("plain  : {:.3}s", time(&plain));
        eprintln!("hooked : {:.3}s", time(&hooked));
        eprintln!("jit off: {:.3}s", time(&jit_off));
    }
}

#[cfg(test)]
mod frame_bench {
    use super::*;

    #[test]
    #[ignore]
    fn geometry_frame_cost() {
        let src = std::fs::read_to_string("examples/geometry.lua").unwrap();
        let s = Script::from_source(&src, "geometry", "B", 1, 1920, 1080).unwrap();
        s.set_mouse(&MouseState { x: 900.0, y: 500.0, inside: true, ..Default::default() }).unwrap();
        for _ in 0..200 {
            drop(s.frame(1.0 / 60.0).unwrap());
        }
        let n = 20_000;
        let t = Instant::now();
        let mut quads = 0;
        for _ in 0..n {
            quads = s.frame(1.0 / 60.0).unwrap().batch.quads();
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / n as f64;
        eprintln!("geometry.lua: {us:.1} us/frame, {quads} quads");
    }

    #[test]
    #[ignore]
    fn per_call_cost() {
        let s = Script::from_source(
            "function draw() for i = 1, 10000 do color(1, 1, 1, 0.5) line(i, 0, i, 100, 1) end end",
            "calls", "B", 1, 1920, 1080,
        ).unwrap();
        for _ in 0..20 {
            drop(s.frame(0.0).unwrap());
        }
        let n = 200;
        let t = Instant::now();
        for _ in 0..n {
            drop(s.frame(0.0).unwrap());
        }
        let ns = t.elapsed().as_secs_f64() * 1e9 / (n as f64 * 10000.0);
        eprintln!("color+line: {ns:.0} ns per shape");
    }
}

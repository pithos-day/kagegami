//! 壁紙スクリプト（LuaJIT）の実行環境。
//!
//! 出力ごとに独立した Lua state を 1 つ持つ。スクリプトに渡すのは画面とマウスの情報だけで、
//! ファイルや OS には触れられないようにしている。

use std::path::Path;
use std::time::{Duration, Instant};

use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, Table, VmState};

use crate::render::{Batch, Blend, color_u8};

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

pub struct Script {
    lua: Lua,
    screen: Table,
    mouse: Table,
    #[allow(dead_code)]
    jit_enabled: bool,
}

/// 壁紙スクリプトに見せないグローバル。ファイル読み込みや別環境への抜け道になるもの。
const REMOVED_GLOBALS: &[&str] = &[
    "dofile", "loadfile", "load", "loadstring", "require", "module",
    "getfenv", "setfenv", "collectgarbage", "gcinfo", "newproxy", "jit",
];

impl Script {
    pub fn load(path: &Path, output: &str, seed: u64, width: u32, height: u32) -> Result<Self, String> {
        let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_source(&src, &path.display().to_string(), output, seed, width, height)
    }

    pub fn from_source(src: &str, chunk_name: &str, output: &str, seed: u64, width: u32, height: u32) -> Result<Self, String> {
        // LuaJIT は jit ライブラリを開いたときに JIT コンパイラーを有効にするので、JIT も読み込む。
        // スクリプトから jit.off() などを呼べないよう、読み込んだ後にグローバルから外す。
        let libs = StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::BIT | StdLib::JIT;
        let lua = Lua::new_with(libs, LuaOptions::default())
            .map_err(|e| e.to_string())?;
        let script = Self::setup(lua, output, seed, width, height).map_err(|e| e.to_string())?;
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

    fn setup(lua: Lua, output: &str, seed: u64, width: u32, height: u32) -> mlua::Result<Self> {
        let g = lua.globals();
        let jit: Table = g.get("jit")?;
        let jit_enabled: bool = jit.get::<Function>("status")?.call(())?;
        if !jit_enabled {
            eprintln!("kagegami: warning: LuaJIT JIT compiler is disabled");
        }
        for name in REMOVED_GLOBALS {
            g.raw_set(*name, mlua::Nil)?;
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

        g.set(
            "clear",
            lua.create_function(|lua, (r, gr, b): (f32, f32, f32)| {
                let mut dl = lua.app_data_mut::<DrawList>().unwrap();
                // clear より前の図形は見えなくなるので捨てる。
                dl.batch.vertices.clear();
                let blend = dl.blend;
                dl.segments.clear();
                dl.segments.push((blend, 0));
                dl.clear = Some([r, gr, b]);
                Ok(())
            })?,
        )?;
        g.set(
            "color",
            lua.create_function(|lua, (r, gr, b, a): (f32, f32, f32, Option<f32>)| {
                lua.app_data_mut::<DrawList>().unwrap().batch.color = color_u8(r, gr, b, a.unwrap_or(1.0));
                Ok(())
            })?,
        )?;
        g.set(
            "blend",
            lua.create_function(|lua, mode: mlua::LuaString| {
                let blend = match mode.as_bytes().as_ref() {
                    b"alpha" => Blend::Alpha,
                    b"add" => Blend::Add,
                    _ => return Err(mlua::Error::runtime("blend: expected \"alpha\" or \"add\"")),
                };
                lua.app_data_mut::<DrawList>().unwrap().set_blend(blend);
                Ok(())
            })?,
        )?;
        g.set(
            "line",
            lua.create_function(|lua, (x1, y1, x2, y2, w): (f32, f32, f32, f32, Option<f32>)| {
                lua.app_data_mut::<DrawList>().unwrap().batch.line(x1, y1, x2, y2, w.unwrap_or(1.0));
                Ok(())
            })?,
        )?;
        g.set(
            "circle",
            lua.create_function(|lua, (x, y, r): (f32, f32, f32)| {
                lua.app_data_mut::<DrawList>().unwrap().batch.circle(x, y, r);
                Ok(())
            })?,
        )?;
        g.set(
            "rect",
            lua.create_function(|lua, (x, y, w, h): (f32, f32, f32, f32)| {
                lua.app_data_mut::<DrawList>().unwrap().batch.rect(x, y, w, h);
                Ok(())
            })?,
        )?;

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

        let s = Self { lua, screen, mouse, jit_enabled };
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
        self.lua.set_app_data(Deadline(Instant::now() + FRAME_BUDGET));
        match ev {
            MouseEvent::Pressed(x, y, b) => self.call("mousepressed", (x, y, b.name())),
            MouseEvent::Released(x, y, b) => self.call("mousereleased", (x, y, b.name())),
        }
        .map_err(|e| e.to_string())
    }

    /// update と draw を呼び、積まれた描画命令を返す。
    pub fn frame(&self, dt: f64) -> Result<mlua::AppDataRef<'_, DrawList>, String> {
        self.lua.set_app_data(Deadline(Instant::now() + FRAME_BUDGET));
        self.lua.app_data_mut::<DrawList>().unwrap().reset();
        self.call("update", dt).map_err(|e| e.to_string())?;
        self.call("draw", ()).map_err(|e| e.to_string())?;
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
        let jit_off = Lua::new();
        jit_off.load("jit.off()").exec().unwrap();
        let st: String = plain.load("local t = {jit.status()} local o = {} for i, v in ipairs(t) do o[#o+1] = tostring(v) end return jit.version .. ' ' .. table.concat(o, ' ')").eval().unwrap();
        eprintln!("status : {st}");
        eprintln!("plain  : {:.3}s", time(&plain));
        eprintln!("hooked : {:.3}s", time(&hooked));
        eprintln!("jit off: {:.3}s", time(&jit_off));
    }
}

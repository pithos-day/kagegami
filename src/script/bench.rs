//! 開発用: hook と JIT の有無による LuaJIT の速さの違いを測る（`cargo test -- --ignored`）。

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
    hooked
        .set_hook(HookTriggers::new().every_nth_instruction(10_000), |_, _| {
            Ok(VmState::Continue)
        })
        .unwrap();
    let sandboxed = Lua::new_with(mlua::StdLib::MATH, mlua::LuaOptions::default()).unwrap();
    eprintln!("no jit lib: {:.3}s", time(&sandboxed));
    let with_jit = Lua::new_with(
        mlua::StdLib::MATH | mlua::StdLib::JIT,
        mlua::LuaOptions::default(),
    )
    .unwrap();
    eprintln!("jit lib  : {:.3}s", time(&with_jit));
    let jit_off = Lua::new();
    jit_off.load("jit.off()").exec().unwrap();
    let st: String = plain.load("local t = {jit.status()} local o = {} for i, v in ipairs(t) do o[#o+1] = tostring(v) end return jit.version .. ' ' .. table.concat(o, ' ')").eval().unwrap();
    eprintln!("status : {st}");
    eprintln!("plain  : {:.3}s", time(&plain));
    eprintln!("hooked : {:.3}s", time(&hooked));
    eprintln!("jit off: {:.3}s", time(&jit_off));
}

//! 開発用: 壁紙スクリプトの 1 フレームと、図形 1 つあたりの処理時間を測る（`cargo test -- --ignored`）。

use super::*;

#[test]
#[ignore]
fn geometry_frame_cost() {
    let src = std::fs::read_to_string("examples/geometry.lua").unwrap();
    let s = Script::from_source(&src, "geometry", "B", 1, (1920, 1080)).unwrap();
    s.set_mouse(&MouseState {
        x: 900.0,
        y: 500.0,
        inside: true,
        ..Default::default()
    })
    .unwrap();
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
        "calls",
        "B",
        1,
        (1920, 1080),
    )
    .unwrap();
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

#[test]
#[ignore]
fn rect_call_cost() {
    for (label, call) in [
        ("rect", "rect(i, 0, 10, 10)"),
        ("rect+angle", "rect(i, 0, 10, 10, 0.5)"),
    ] {
        let s = Script::from_source(
            &format!("function draw() for i = 1, 10000 do color(1, 1, 1, 0.5) {call} end end"),
            "calls",
            "B",
            1,
            (1920, 1080),
        )
        .unwrap();
        for _ in 0..20 {
            drop(s.frame(0.0).unwrap());
        }
        let n = 500;
        let t = Instant::now();
        for _ in 0..n {
            drop(s.frame(0.0).unwrap());
        }
        let ns = t.elapsed().as_secs_f64() * 1e9 / (n as f64 * 10000.0);
        eprintln!("color+{label}: {ns:.1} ns per shape");
    }
}

/// 描画関数ごとの 1 回の呼び出しにかかる時間。
#[test]
#[ignore]
fn draw_call_cost() {
    for (label, call) in [
        ("color", "color(i * 1e-4, 1, 1, 0.5)"),
        ("circle", "circle(i, 0, 2)"),
        ("line", "line(i, 0, i, 10, 1)"),
        ("rect", "rect(i, 0, 10, 10)"),
        ("rect+angle", "rect(i, 0, 10, 10, 0.5)"),
        ("clear", "clear(0, 0, 0)"),
        ("blend", "blend('alpha')"),
    ] {
        let s = Script::from_source(
            &format!("function draw() for i = 1, 10000 do {call} end end"),
            "calls",
            "B",
            1,
            (1920, 1080),
        )
        .unwrap();
        for _ in 0..20 {
            drop(s.frame(0.0).unwrap());
        }
        let n = 500;
        let t = Instant::now();
        for _ in 0..n {
            drop(s.frame(0.0).unwrap());
        }
        let ns = t.elapsed().as_secs_f64() * 1e9 / (n as f64 * 10000.0);
        eprintln!("{label}: {ns:.2} ns per call");
    }
}

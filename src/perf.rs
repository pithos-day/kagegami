//! `--perf`: 壁紙の負荷を 1 秒ごとに標準エラー出力へ出す（#41）。
//!
//! 壁紙を作りながら、制限（Lua の時間・図形の数・シェーダーの GPU 時間）に対する余裕を見るためのもの。
//! 測ることで描画を変えないように、`glFinish` は使わず、GPU 時間は timer query で測る。

use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// 表示を更新する間隔。
pub const INTERVAL: Duration = Duration::from_secs(1);

/// 平均と最大を取る時間の集計。
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Timing {
    /// 今の間隔の合計と数（平均に使う）。
    sum: Duration,
    count: u32,
    /// 読み込んでからの最大。
    max: Duration,
}

impl Timing {
    fn record(&mut self, d: Duration) {
        self.sum += d;
        self.count += 1;
        self.max = self.max.max(d);
    }

    fn avg(&self) -> Option<Duration> {
        (self.count > 0).then(|| self.sum / self.count)
    }
}

/// 出力ごとの負荷の集計。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct OutputStats {
    /// 今の間隔に描いたフレームの数。
    frames: u32,
    lua: Timing,
    gpu: Timing,
    /// 今の間隔で最も多かった、1 フレームの図形の数。
    quads: usize,
}

impl OutputStats {
    pub fn frame(&mut self) {
        self.frames += 1;
    }

    pub fn lua(&mut self, d: Duration, quads: usize) {
        self.lua.record(d);
        self.quads = self.quads.max(quads);
    }

    pub fn gpu(&mut self, times: &[Duration]) {
        for d in times {
            self.gpu.record(*d);
        }
    }

    /// 壁紙を読み直したときに呼ぶ。最大値も含めて消す。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 1 間隔分を表示に使えるように取り出し、次の間隔のために平均と数を消す（最大は残す）。
    fn take(&mut self) -> Self {
        let snapshot = self.clone();
        self.frames = 0;
        self.quads = 0;
        for t in [&mut self.lua, &mut self.gpu] {
            t.sum = Duration::ZERO;
            t.count = 0;
        }
        snapshot
    }
}

/// 壁紙の種類。表示する欄が変わる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Lua,
    Shader,
}

/// 制限の値。割合の計算に使う。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub lua: Duration,
    pub quads: usize,
    pub shader_gpu: Duration,
}

/// 表示する 1 出力分の値。
#[derive(Debug, Clone, PartialEq)]
pub struct OutputRow {
    pub name: String,
    pub kind: Kind,
    /// 壁紙がエラーで止まっている。
    pub stopped: bool,
    pub fps: f64,
    stats: OutputStats,
}

/// プロセス全体の値。
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessRow {
    /// 1 コアを 100% とした CPU 使用率。測れなければ None。
    pub cpu_percent: Option<f64>,
    /// GPU のエンジンごとの使用率（%）。エンジンの名前の順。DRM の fdinfo から取れなければ None。
    pub gpu_engines: Option<Vec<(String, f64)>>,
    pub rss_bytes: Option<u64>,
    /// Lua の壁紙がなければ None。
    pub lua_bytes: Option<usize>,
}

/// 出力ごとの集計から、1 間隔分の表示の値を作る。
pub fn take_row(
    name: &str,
    kind: Kind,
    stopped: bool,
    stats: &mut OutputStats,
    elapsed: Duration,
) -> OutputRow {
    let stats = stats.take();
    let secs = elapsed.as_secs_f64();
    OutputRow {
        name: name.to_string(),
        kind,
        stopped,
        fps: if secs > 0.0 {
            stats.frames as f64 / secs
        } else {
            0.0
        },
        stats,
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn percent(d: Duration, limit: Duration) -> f64 {
    d.as_secs_f64() / limit.as_secs_f64() * 100.0
}

/// `--fps` の値。整数ならそのまま、そうでなければ小数 1 桁で書く。
fn fmt_target(fps: f64) -> String {
    if fps.fract() == 0.0 {
        format!("{fps:.0}")
    } else {
        format!("{fps:.1}")
    }
}

/// 3 桁ごとにカンマを入れる。
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// TTY に出す行。出力ごとに 1 行と、最後にプロセス全体の 1 行。桁の幅をそろえる。
pub fn format_tty(
    rows: &[OutputRow],
    process: &ProcessRow,
    target_fps: f64,
    limits: &Limits,
) -> Vec<String> {
    let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let target = fmt_target(target_fps);
    let mut lines = Vec::with_capacity(rows.len() + 1);
    for r in rows {
        let mut l = format!("{:<name_width$}  ", r.name);
        if r.stopped {
            l.push_str("stopped (error)");
            lines.push(l);
            continue;
        }
        let _ = write!(l, "{:>6.1}/{target} fps", r.fps);
        let s = &r.stats;
        if r.kind == Kind::Lua {
            match s.lua.avg() {
                Some(avg) => {
                    let _ = write!(
                        l,
                        "  lua {:>6.2} ms (max {:>6.2} ms, {:>3.0}%)",
                        ms(avg),
                        ms(s.lua.max),
                        percent(s.lua.max, limits.lua)
                    );
                }
                None => l.push_str("  lua      - ms (max      - ms,    -)"),
            }
        }
        // timer query がない環境では GPU 時間の欄を出さない。
        if let Some(avg) = s.gpu.avg() {
            let _ = write!(
                l,
                "  gpu {:>6.2} ms (max {:>6.2} ms",
                ms(avg),
                ms(s.gpu.max)
            );
            if r.kind == Kind::Shader {
                let _ = write!(l, ", {:>3.0}%", percent(s.gpu.max, limits.shader_gpu));
            }
            l.push(')');
        }
        if r.kind == Kind::Lua && s.frames > 0 {
            let _ = write!(
                l,
                "  quads {:>9} ({:>3.0}%)",
                group(s.quads),
                s.quads as f64 / limits.quads as f64 * 100.0
            );
        }
        lines.push(l);
    }
    let mut l = String::new();
    match process.cpu_percent {
        Some(c) => {
            let _ = write!(l, "cpu {c:>3.0}%");
        }
        None => l.push_str("cpu   -%"),
    }
    if let Some(engines) = process.gpu_engines.as_deref().filter(|e| !e.is_empty()) {
        l.push_str("  gpu");
        for (i, (name, p)) in engines.iter().enumerate() {
            let sep = if i == 0 { " " } else { "  " };
            let _ = write!(l, "{sep}{name} {p:>3.0}%");
        }
    }
    if let Some(b) = process.rss_bytes {
        let _ = write!(l, "  rss {:>6.1} MiB", mib(b));
    }
    if let Some(b) = process.lua_bytes {
        let _ = write!(l, "  lua {:>6.1} MiB", mib(b as u64));
    }
    lines.push(l);
    lines
}

/// TTY でないときに出す 1 行（logfmt）。
pub fn format_logfmt(
    rows: &[OutputRow],
    process: &ProcessRow,
    target_fps: f64,
    limits: &Limits,
) -> String {
    let target = fmt_target(target_fps);
    let mut parts: Vec<String> = Vec::new();
    for r in rows {
        parts.push(format!("out={}", r.name));
        if r.stopped {
            parts.push("status=stopped".into());
            continue;
        }
        parts.push(format!("fps={:.1}/{target}", r.fps));
        let s = &r.stats;
        if r.kind == Kind::Lua
            && let Some(avg) = s.lua.avg()
        {
            parts.push(format!("lua_avg={:.2}ms", ms(avg)));
            parts.push(format!("lua_max={:.2}ms", ms(s.lua.max)));
            parts.push(format!("lua_limit={:.0}%", percent(s.lua.max, limits.lua)));
        }
        if let Some(avg) = s.gpu.avg() {
            parts.push(format!("gpu_avg={:.2}ms", ms(avg)));
            parts.push(format!("gpu_max={:.2}ms", ms(s.gpu.max)));
            if r.kind == Kind::Shader {
                parts.push(format!(
                    "gpu_limit={:.0}%",
                    percent(s.gpu.max, limits.shader_gpu)
                ));
            }
        }
        if r.kind == Kind::Lua && s.frames > 0 {
            parts.push(format!("quads={}", s.quads));
            parts.push(format!(
                "quads_limit={:.0}%",
                s.quads as f64 / limits.quads as f64 * 100.0
            ));
        }
    }
    if let Some(c) = process.cpu_percent {
        parts.push(format!("cpu={c:.0}%"));
    }
    for (name, p) in process.gpu_engines.iter().flatten() {
        parts.push(format!("gpu_{name}={p:.0}%"));
    }
    if let Some(b) = process.rss_bytes {
        parts.push(format!("rss={:.1}MiB", mib(b)));
    }
    if let Some(b) = process.lua_bytes {
        parts.push(format!("lua_mem={:.1}MiB", mib(b as u64)));
    }
    parts.join(" ")
}

/// `/proc/self/stat` から、ユーザーとカーネルで使った CPU 時間の合計（clock tick）を読む。
/// 2 番目の欄（コマンド名）は空白や括弧を含みうるので、最後の `)` より後ろを数える。
fn parse_cpu_ticks(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    // `)` の後ろは 3 番目の欄（state）から始まる。utime は 14 番目、stime は 15 番目。
    let mut fields = rest.split_whitespace().skip(11);
    let utime: u64 = fields.next()?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some(utime + stime)
}

/// `/proc/self/statm` から RSS のページ数を読む（2 番目の欄）。
fn parse_rss_pages(statm: &str) -> Option<u64> {
    statm.split_whitespace().nth(1)?.parse().ok()
}

/// プロセスの CPU 使用率と RSS を測る。
pub struct ProcessMeter {
    last: Option<(u64, Instant)>,
    ticks_per_sec: f64,
    page_size: u64,
}

impl ProcessMeter {
    pub fn new() -> Self {
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let mut m = Self {
            last: None,
            ticks_per_sec: if ticks > 0 { ticks as f64 } else { 100.0 },
            page_size: if page > 0 { page as u64 } else { 4096 },
        };
        m.last = m.cpu_ticks().map(|t| (t, Instant::now()));
        m
    }

    fn cpu_ticks(&self) -> Option<u64> {
        parse_cpu_ticks(&std::fs::read_to_string("/proc/self/stat").ok()?)
    }

    /// 前に測ってからの CPU 使用率（%）と、今の RSS（バイト）。
    pub fn sample(&mut self, now: Instant) -> (Option<f64>, Option<u64>) {
        let ticks = self.cpu_ticks();
        let cpu = match (self.last, ticks) {
            (Some((t0, at)), Some(t1)) if now > at => {
                let used = t1.saturating_sub(t0) as f64 / self.ticks_per_sec;
                Some(used / now.duration_since(at).as_secs_f64() * 100.0)
            }
            _ => None,
        };
        self.last = ticks.map(|t| (t, now));
        let rss = std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| parse_rss_pages(&s))
            .map(|p| p * self.page_size);
        (cpu, rss)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        lua: Duration::from_millis(250),
        quads: 1_000_000,
        shader_gpu: Duration::from_millis(250),
    };

    fn lua_stats() -> OutputStats {
        let mut s = OutputStats::default();
        for (ms, q) in [(2, 10_000), (4, 12_345), (3, 11_000)] {
            s.frame();
            s.lua(Duration::from_millis(ms), q);
        }
        s.gpu(&[Duration::from_millis(1), Duration::from_millis(2)]);
        s
    }

    fn row(name: &str, kind: Kind, stats: &mut OutputStats) -> OutputRow {
        take_row(name, kind, false, stats, Duration::from_secs(1))
    }

    const PROCESS: ProcessRow = ProcessRow {
        cpu_percent: Some(12.4),
        gpu_engines: None,
        rss_bytes: Some(48 * 1024 * 1024 + 200 * 1024),
        lua_bytes: Some(2 * 1024 * 1024 + 100 * 1024),
    };

    #[test]
    fn take_keeps_max_and_clears_the_rest() {
        let mut s = lua_stats();
        let r = row("DP-1", Kind::Lua, &mut s);
        assert_eq!(r.fps, 3.0);
        assert_eq!(r.stats.lua.avg(), Some(Duration::from_millis(3)));
        assert_eq!(r.stats.quads, 12_345);
        // 次の間隔では、平均と数は消え、最大だけが残る。
        assert_eq!(s.frames, 0);
        assert_eq!(s.lua.avg(), None);
        assert_eq!(s.gpu.avg(), None);
        assert_eq!(s.quads, 0);
        assert_eq!(s.lua.max, Duration::from_millis(4));
        assert_eq!(s.gpu.max, Duration::from_millis(2));
        // 読み直したら最大も消す。
        s.reset();
        assert_eq!(s, OutputStats::default());
    }

    #[test]
    fn tty_lines_are_aligned() {
        let mut a = lua_stats();
        let mut b = lua_stats();
        let rows = [
            row("DP-1", Kind::Lua, &mut a),
            row("HDMI-A-1", Kind::Lua, &mut b),
        ];
        let lines = format_tty(&rows, &PROCESS, 60.0, &LIMITS);
        assert_eq!(
            lines,
            [
                "DP-1         3.0/60 fps  lua   3.00 ms (max   4.00 ms,   2%)  gpu   1.50 ms (max   2.00 ms)  quads    12,345 (  1%)",
                "HDMI-A-1     3.0/60 fps  lua   3.00 ms (max   4.00 ms,   2%)  gpu   1.50 ms (max   2.00 ms)  quads    12,345 (  1%)",
                "cpu  12%  rss   48.2 MiB  lua    2.1 MiB",
            ]
        );
    }

    #[test]
    fn tty_shader_has_gpu_limit_and_no_lua() {
        let mut s = OutputStats::default();
        s.frame();
        s.gpu(&[Duration::from_millis(25)]);
        let rows = [row("DP-1", Kind::Shader, &mut s)];
        let process = ProcessRow {
            lua_bytes: None,
            ..PROCESS
        };
        let lines = format_tty(&rows, &process, 59.5, &LIMITS);
        assert_eq!(
            lines,
            [
                "DP-1     1.0/59.5 fps  gpu  25.00 ms (max  25.00 ms,  10%)",
                "cpu  12%  rss   48.2 MiB",
            ]
        );
    }

    #[test]
    fn tty_omits_gpu_without_timer_and_marks_stopped() {
        let mut s = OutputStats::default();
        s.frame();
        s.lua(Duration::from_millis(1), 1);
        let mut stopped = OutputStats::default();
        let rows = [
            row("DP-1", Kind::Lua, &mut s),
            take_row(
                "HDMI-A-1",
                Kind::Lua,
                true,
                &mut stopped,
                Duration::from_secs(1),
            ),
        ];
        let lines = format_tty(&rows, &PROCESS, 60.0, &LIMITS);
        assert_eq!(
            lines[0],
            "DP-1         1.0/60 fps  lua   1.00 ms (max   1.00 ms,   0%)  quads         1 (  0%)"
        );
        assert_eq!(lines[1], "HDMI-A-1  stopped (error)");
    }

    #[test]
    fn tty_without_frames_shows_dashes() {
        let mut s = OutputStats::default();
        let rows = [row("DP-1", Kind::Lua, &mut s)];
        let lines = format_tty(&rows, &PROCESS, 60.0, &LIMITS);
        assert_eq!(
            lines[0],
            "DP-1     0.0/60 fps  lua      - ms (max      - ms,    -)"
        );
    }

    #[test]
    fn logfmt_line() {
        let mut a = lua_stats();
        let mut b = OutputStats::default();
        b.frame();
        b.gpu(&[Duration::from_millis(25)]);
        let mut c = OutputStats::default();
        let rows = [
            row("DP-1", Kind::Lua, &mut a),
            row("HDMI-A-1", Kind::Shader, &mut b),
            take_row("eDP-1", Kind::Lua, true, &mut c, Duration::from_secs(1)),
        ];
        assert_eq!(
            format_logfmt(&rows, &PROCESS, 60.0, &LIMITS),
            "out=DP-1 fps=3.0/60 lua_avg=3.00ms lua_max=4.00ms lua_limit=2% gpu_avg=1.50ms gpu_max=2.00ms \
             quads=12345 quads_limit=1% out=HDMI-A-1 fps=1.0/60 gpu_avg=25.00ms gpu_max=25.00ms gpu_limit=10% \
             out=eDP-1 status=stopped cpu=12% rss=48.2MiB lua_mem=2.1MiB"
        );
    }

    #[test]
    fn gpu_engines_follow_cpu() {
        let process = ProcessRow {
            gpu_engines: Some(vec![("compute".into(), 0.4), ("gfx".into(), 8.2)]),
            ..PROCESS
        };
        let mut s = lua_stats();
        let rows = [row("DP-1", Kind::Lua, &mut s)];
        let lines = format_tty(&rows, &process, 60.0, &LIMITS);
        assert_eq!(
            lines.last().unwrap(),
            "cpu  12%  gpu compute   0%  gfx   8%  rss   48.2 MiB  lua    2.1 MiB"
        );
        let l = format_logfmt(&rows, &process, 60.0, &LIMITS);
        assert!(
            l.ends_with("cpu=12% gpu_compute=0% gpu_gfx=8% rss=48.2MiB lua_mem=2.1MiB"),
            "{l}"
        );
        // 取れた値が空（どのエンジンも使っていない）なら出さない。
        let idle = ProcessRow {
            gpu_engines: Some(Vec::new()),
            ..PROCESS
        };
        let lines = format_tty(&rows, &idle, 60.0, &LIMITS);
        assert_eq!(
            lines.last().unwrap(),
            "cpu  12%  rss   48.2 MiB  lua    2.1 MiB"
        );
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1000), "1,000");
        assert_eq!(group(1_000_000), "1,000,000");
    }

    #[test]
    fn parses_proc_stat() {
        // コマンド名に空白と括弧が入っていても読める。
        let stat =
            "1234 (kage gami) (x)) S 1 1234 1234 0 -1 4194304 100 0 0 0 250 50 0 0 20 0 3 0 1000 0";
        assert_eq!(parse_cpu_ticks(stat), Some(300));
        assert_eq!(parse_cpu_ticks("garbage"), None);
        assert_eq!(parse_rss_pages("5000 1234 300 1 0 800 0"), Some(1234));
        assert_eq!(parse_rss_pages(""), None);
    }

    #[test]
    fn reads_own_process() {
        let mut m = ProcessMeter::new();
        let (_, rss) = m.sample(Instant::now() + Duration::from_millis(10));
        assert!(rss.unwrap() > 0);
        assert!(m.last.is_some());
    }
}

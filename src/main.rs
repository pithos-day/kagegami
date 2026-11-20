mod app;
mod cli;
mod egl;
mod render;
mod script;

use std::os::fd::AsRawFd;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::CompositorState, output::OutputState, registry::RegistryState, seat::SeatState,
    shell::wlr_layer::LayerShell,
};
use wayland_client::{Connection, globals::registry_queue_init};

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::ParseResult::Run(a)) => a,
        Ok(cli::ParseResult::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("kagegami: {e}\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kagegami: {e}");
            ExitCode::FAILURE
        }
    }
}

/// スクリプトがこの時間戻ってこなければ終了する。
const WATCHDOG_LIMIT: Duration = Duration::from_secs(5);
/// ファイルの更新を確認する間隔。
const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// 更新日時とサイズでファイルの変更を検出する。
/// エディターは別名で保存して置き換えることが多いので、inotify ではなく定期的に確認する。
struct FileWatcher {
    path: std::path::PathBuf,
    last: Option<(std::time::SystemTime, u64)>,
    next_check: Instant,
}

impl FileWatcher {
    fn new(path: std::path::PathBuf) -> Self {
        let last = Self::stamp(&path);
        Self { path, last, next_check: Instant::now() + WATCH_INTERVAL }
    }

    fn stamp(path: &std::path::Path) -> Option<(std::time::SystemTime, u64)> {
        let m = std::fs::metadata(path).ok()?;
        Some((m.modified().ok()?, m.len()))
    }

    fn until_next(&self, now: Instant) -> Duration {
        self.next_check.saturating_duration_since(now)
    }

    fn changed(&mut self, now: Instant) -> bool {
        if now < self.next_check {
            return false;
        }
        self.next_check = now + WATCH_INTERVAL;
        let stamp = Self::stamp(&self.path);
        // 保存の途中で一瞬消えている場合は、次の確認まで待つ。
        if stamp.is_none() || stamp == self.last {
            return false;
        }
        self.last = stamp;
        true
    }
}

fn run(args: cli::Args) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("cannot connect to Wayland: {e}"))?;
    let (globals, mut queue) = registry_queue_init(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(|e| format!("wlr-layer-shell: {e}"))?;
    let egl = egl::EglContext::new(conn.backend().display_ptr().cast())?;

    let mut app = app::App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        pointer: None,
        compositor,
        layer_shell,
        egl,
        renderer: None,
        frame_period: Duration::from_secs_f64(1.0 / args.fps),
        args,
        wallpapers: Vec::new(),
    };

    script::spawn_watchdog(WATCHDOG_LIMIT);
    let mut watcher = FileWatcher::new(app.args.script.clone());
    loop {
        queue.dispatch_pending(&mut app).map_err(|e| e.to_string())?;
        if watcher.changed(Instant::now()) {
            app.reload();
        }
        app.render_due(&qh, Instant::now());
        queue.flush().map_err(|e| e.to_string())?;

        let Some(guard) = queue.prepare_read() else { continue };
        let now = Instant::now();
        let timeout = app.next_timeout(now).map_or(watcher.until_next(now), |t| t.min(watcher.until_next(now)));
        let timeout = Some(timeout);
        let mut pfd = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = timeout.map_or(-1, |d| d.as_millis().min(i32::MAX as u128) as i32 + i32::from(d.subsec_micros() % 1000 != 0));
        let n = unsafe { libc::poll(&mut pfd, 1, ms) };
        if n > 0 {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(format!("Wayland connection: {e}")),
            }
        } else {
            drop(guard);
        }
    }
}

mod app;
#[cfg(feature = "gpu-bench")]
mod bench;
mod buffers;
mod cli;
mod cubemap_pass;
mod directive;
mod egl;
mod gl_program;
#[cfg(test)]
mod gpu_test;
mod gpu_usage;
mod image;
mod keyboard;
mod lua_images;
mod multipass;
mod noise;
mod perf;
mod pointer;
mod render;
mod script;
mod shader;
mod source;
mod stderr;
mod texture;
mod watcher;
mod zipfile;

use std::os::fd::AsRawFd;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use wayland_client::backend::{ReadEventsGuard, WaylandError};
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
            stderr::elog!("kagegami: {e}");
            ExitCode::FAILURE
        }
    }
}

/// スクリプトがこの時間戻ってこなければ終了する。
const WATCHDOG_LIMIT: Duration = Duration::from_secs(5);

fn run(args: cli::Args) -> Result<(), String> {
    let source = source::Source::open(&args.script)?;
    let watched = source.watched_entry();
    let conn =
        Connection::connect_to_env().map_err(|e| format!("cannot connect to Wayland: {e}"))?;
    let (globals, mut queue) = registry_queue_init(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let mut app = app::App::new(&conn, &globals, &qh, args, source)?;

    script::spawn_watchdog(WATCHDOG_LIMIT);
    let mut watcher = watcher::FileWatcher::new(watched);
    loop {
        queue
            .dispatch_pending(&mut app)
            .map_err(|e| e.to_string())?;
        if let Some(paths) = app.take_watch_paths() {
            watcher.set_paths(paths);
        }
        if watcher.changed(Instant::now()) {
            app.reload();
        }
        app.render_due(&qh, Instant::now());
        app.perf_tick(Instant::now());
        queue.flush().map_err(|e| e.to_string())?;

        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let now = Instant::now();
        let timeout = [app.next_timeout(now), app.perf_timeout(now)]
            .into_iter()
            .flatten()
            .fold(watcher.until_next(now), Duration::min);
        wait_and_read(guard, timeout)?;
    }
}

/// Wayland の接続にイベントが届くか `timeout` が過ぎるまで待ち、届いたイベントを読む。
fn wait_and_read(guard: ReadEventsGuard, timeout: Duration) -> Result<(), String> {
    let mut pfd = libc::pollfd {
        fd: guard.connection_fd().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let n = unsafe { libc::poll(&mut pfd, 1, poll_timeout_ms(timeout)) };
    if n <= 0 {
        return Ok(());
    }
    match guard.read() {
        Ok(_) => Ok(()),
        Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(format!("Wayland connection: {e}")),
    }
}

/// `poll` に渡す待ち時間（ミリ秒）。1 ミリ秒未満の端数は切り上げる（早く起きすぎて空回りしないように）。
fn poll_timeout_ms(timeout: Duration) -> i32 {
    timeout.as_millis().min(i32::MAX as u128) as i32
        + i32::from(!timeout.subsec_micros().is_multiple_of(1000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_timeout_rounds_up_to_milliseconds() {
        assert_eq!(poll_timeout_ms(Duration::ZERO), 0);
        assert_eq!(poll_timeout_ms(Duration::from_millis(16)), 16);
        assert_eq!(poll_timeout_ms(Duration::from_micros(1)), 1);
        assert_eq!(poll_timeout_ms(Duration::from_micros(16_001)), 17);
        assert_eq!(poll_timeout_ms(Duration::from_micros(16_999)), 17);
    }
}

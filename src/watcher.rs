//! 壁紙のファイルの変更の検出。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// ファイルの更新を確認する間隔。
pub const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// 更新日時とサイズでファイルの変更を検出する。
/// エディターは別名で保存して置き換えることが多いので、inotify ではなく定期的に確認する。
pub struct FileWatcher {
    path: PathBuf,
    last: Option<(SystemTime, u64)>,
    next_check: Instant,
}

impl FileWatcher {
    pub fn new(path: PathBuf) -> Self {
        let last = Self::stamp(&path);
        Self {
            path,
            last,
            next_check: Instant::now() + WATCH_INTERVAL,
        }
    }

    fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
        let m = std::fs::metadata(path).ok()?;
        Some((m.modified().ok()?, m.len()))
    }

    pub fn until_next(&self, now: Instant) -> Duration {
        self.next_check.saturating_duration_since(now)
    }

    /// 前の確認から変わっていれば true。確認する時刻になっていなければ確かめずに false を返す。
    pub fn changed(&mut self, now: Instant) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// テストごとに別のファイルを使う。
    fn temp_file(name: &str, content: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("kagegami-watcher-{}-{name}", std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn unchanged_file_is_not_reported() {
        let path = temp_file("unchanged", "a");
        let mut w = FileWatcher::new(path.clone());
        assert!(!w.changed(Instant::now() + WATCH_INTERVAL));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn changed_file_is_reported_once_after_the_interval() {
        let path = temp_file("changed", "a");
        let mut w = FileWatcher::new(path.clone());
        // new の後に測るので、start + WATCH_INTERVAL は最初の確認の時刻以降になる。
        let start = Instant::now();
        std::fs::write(&path, "ab").unwrap();
        // 確認する時刻の前は確かめない。
        assert!(!w.changed(start));
        let t = start + WATCH_INTERVAL;
        assert!(w.changed(t));
        // 同じ変更は 2 度報告しない。
        assert!(!w.changed(t + WATCH_INTERVAL));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_file_waits_for_the_next_check() {
        let path = temp_file("missing", "a");
        let mut w = FileWatcher::new(path.clone());
        // new の後に測るので、start + WATCH_INTERVAL は最初の確認の時刻以降になる。
        let start = Instant::now();
        std::fs::remove_file(&path).unwrap();
        // 保存の途中で消えている間は報告しない。
        assert!(!w.changed(start + WATCH_INTERVAL));
        std::fs::write(&path, "abc").unwrap();
        assert!(w.changed(start + WATCH_INTERVAL * 2));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn until_next_counts_down_to_the_check() {
        let path = temp_file("until", "a");
        let w = FileWatcher::new(path.clone());
        let start = Instant::now();
        assert!(w.until_next(start) <= WATCH_INTERVAL);
        assert_eq!(w.until_next(start + WATCH_INTERVAL * 2), Duration::ZERO);
        std::fs::remove_file(path).unwrap();
    }
}

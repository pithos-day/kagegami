//! 壁紙のファイルと、壁紙が使う画像の変更の検出。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// ファイルの更新を確認する間隔。
pub const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// 更新日時とサイズでファイルの変更を検出する。
/// エディターは別名で保存して置き換えることが多いので、inotify ではなく定期的に確認する。
pub struct FileWatcher {
    /// 見張るファイルと、前に確認したときの (更新日時, サイズ)。
    files: Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    next_check: Instant,
}

impl FileWatcher {
    pub fn new(path: PathBuf) -> Self {
        let last = Self::stamp(&path);
        Self {
            files: vec![(path, last)],
            next_check: Instant::now() + WATCH_INTERVAL,
        }
    }

    /// 見張るファイルを入れ替える。前から見張っているファイルは、前に確認したときの状態を引き継ぐ
    /// （入れ替えるまでの変更を見落とさないため）。新しいファイルは今の状態から見張る。
    pub fn set_paths(&mut self, paths: Vec<PathBuf>) {
        let old = std::mem::take(&mut self.files);
        self.files = paths
            .into_iter()
            .map(|p| {
                let last = match old.iter().find(|(q, _)| *q == p) {
                    Some((_, last)) => *last,
                    None => Self::stamp(&p),
                };
                (p, last)
            })
            .collect();
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
        let mut changed = false;
        for (path, last) in &mut self.files {
            let stamp = Self::stamp(path);
            // 保存の途中で一瞬消えている場合は、次の確認まで待つ。
            if stamp.is_none() || stamp == *last {
                continue;
            }
            *last = stamp;
            changed = true;
        }
        changed
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
    fn added_files_are_watched_from_their_current_state() {
        let script = temp_file("multi-script", "a");
        let image = temp_file("multi-image", "1");
        let missing = std::env::temp_dir().join(format!(
            "kagegami-watcher-{}-multi-missing",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);
        let mut w = FileWatcher::new(script.clone());
        let start = Instant::now();
        w.set_paths(vec![script.clone(), image.clone(), missing.clone()]);
        // 足しただけでは報告しない。
        assert!(!w.changed(start + WATCH_INTERVAL));
        // 画像が変わったら報告する。
        std::fs::write(&image, "12").unwrap();
        assert!(w.changed(start + WATCH_INTERVAL * 2));
        // 読み込めなかった画像が現れたら報告する（直して保存したら読み直すため）。
        std::fs::write(&missing, "x").unwrap();
        assert!(w.changed(start + WATCH_INTERVAL * 3));
        // 見張るのをやめたファイルは報告しない。
        w.set_paths(vec![script.clone()]);
        std::fs::write(&image, "123").unwrap();
        assert!(!w.changed(start + WATCH_INTERVAL * 4));
        for p in [script, image, missing] {
            std::fs::remove_file(p).unwrap();
        }
    }

    #[test]
    fn set_paths_keeps_the_state_of_files_already_watched() {
        let script = temp_file("keep-script", "a");
        let mut w = FileWatcher::new(script.clone());
        let start = Instant::now();
        // 入れ替える前の変更も、入れ替えた後に報告する。
        std::fs::write(&script, "ab").unwrap();
        w.set_paths(vec![script.clone()]);
        assert!(w.changed(start + WATCH_INTERVAL));
        std::fs::remove_file(script).unwrap();
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

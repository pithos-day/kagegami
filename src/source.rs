//! 壁紙のファイルの在りか（#60）。kagegami に渡したパスが、ファイル・ディレクトリ・zip ファイルの
//! どれでも、壁紙のファイルと、そこから指定されたファイル（バッファー・Common・画像）を同じ形で読む。
//!
//! - ファイル: そのファイルが入口。指定されたファイルはディスクから読む
//! - ディレクトリ: 直下の `main.glsl`・`main.frag`・`main.lua` のどれか 1 つが入口
//! - zip ファイル: 直下の入口を探し、なければ、直下がディレクトリ 1 つだけのときその中を探す。
//!   zip の中のファイルは `<zip のパス>/<zip の中のパス>` というパスで表し、zip の外は読まない。
//!   展開するのは読んだファイルだけで、その合計に上限（`ZIP_EXTRACT_LIMIT`）を置く

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use crate::zipfile::Archive;

/// ディレクトリ・zip の中の入口のファイルの名前。
pub const ENTRY_NAMES: [&str; 3] = ["main.glsl", "main.frag", "main.lua"];

/// zip から展開するファイルの、展開した後のサイズの合計の上限。
pub const ZIP_EXTRACT_LIMIT: u64 = 256 * 1024 * 1024;

pub struct Source {
    /// kagegami に渡したパス。
    arg: PathBuf,
    /// 入口のファイル。zip の中なら `<zip のパス>/<zip の中のパス>`。
    entry: PathBuf,
    /// zip ファイルを渡したときだけ Some。
    zip: Option<Zip>,
}

struct Zip {
    /// 読み直すたびに読む。読めなければそのエラー。
    archive: Result<Archive, String>,
    /// この読み直しで展開したファイルと、その展開した後のサイズ。出力ごとに読んでも 1 回と数える。
    extracted: Mutex<HashMap<String, u64>>,
}

impl Zip {
    fn read(path: &Path) -> Self {
        let archive = Archive::open(path).map_err(|e| format!("{}: {e}", path.display()));
        Self {
            archive,
            extracted: Mutex::new(HashMap::new()),
        }
    }
}

fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

fn list_names() -> String {
    ENTRY_NAMES.join(", ")
}

/// `names` のうち入口の名前のもの。2 つ以上ならエラー。
fn pick_entry<'a>(
    names: impl Iterator<Item = &'a str>,
    place: &str,
) -> Result<Option<&'a str>, String> {
    let found: Vec<_> = names.filter(|n| ENTRY_NAMES.contains(n)).collect();
    match found[..] {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        _ => Err(format!(
            "{place}: more than one entry file ({}); keep only one of {}",
            found.join(", "),
            list_names()
        )),
    }
}

/// zip の中の入口のパス。
fn find_zip_entry(archive: &Archive, zip: &Path) -> Result<String, String> {
    let place = zip.display().to_string();
    let names = || archive.entries().iter().map(|e| e.name.as_str());
    if let Some(n) = pick_entry(names(), &place)? {
        return Ok(n.to_string());
    }
    // 直下の名前（`ink/main.glsl` なら `ink/`、`main.glsl` なら `main.glsl`）。
    let mut tops: Vec<String> = names()
        .filter_map(|n| match n.split_once('/') {
            Some((dir, _)) => Some(format!("{dir}/")),
            None => (!n.is_empty()).then(|| n.to_string()),
        })
        .collect();
    tops.sort();
    tops.dedup();
    if let [dir] = &tops[..]
        && dir.ends_with('/')
    {
        let inner = names().filter_map(|n| n.strip_prefix(dir.as_str()));
        if let Some(n) = pick_entry(inner, &format!("{place}: {dir}"))? {
            return Ok(format!("{dir}{n}"));
        }
    }
    Err(format!(
        "{place}: no entry file ({}) in the zip file or in its only top-level directory",
        list_names()
    ))
}

impl Source {
    /// kagegami に渡したパスから入口を決める。起動時に呼び、エラーなら終了する（#37）。
    pub fn open(arg: &Path) -> Result<Self, String> {
        let fail = |e: std::io::Error| format!("{}: {e}", arg.display());
        if arg.is_dir() {
            let mut names = Vec::new();
            for entry in std::fs::read_dir(arg).map_err(fail)? {
                let entry = entry.map_err(fail)?;
                if entry.path().is_file()
                    && let Some(n) = entry.file_name().to_str()
                {
                    names.push(n.to_string());
                }
            }
            names.sort();
            let entry = pick_entry(names.iter().map(String::as_str), &arg.display().to_string())?
                .ok_or_else(|| {
                format!(
                    "{}: no entry file ({}) in the directory",
                    arg.display(),
                    list_names()
                )
            })?;
            let entry = arg.join(entry);
            check_readable(&entry)?;
            return Ok(Self {
                arg: arg.to_path_buf(),
                entry,
                zip: None,
            });
        }
        if is_zip_path(arg) {
            let zip = Zip::read(arg);
            let archive = zip.archive.as_ref().map_err(Clone::clone)?;
            let name = find_zip_entry(archive, arg)?;
            return Ok(Self {
                arg: arg.to_path_buf(),
                entry: arg.join(name),
                zip: Some(zip),
            });
        }
        check_readable(arg)?;
        Ok(Self {
            arg: arg.to_path_buf(),
            entry: arg.to_path_buf(),
            zip: None,
        })
    }

    /// ディスクから読む `Source`（テスト用。入口は空のパス）。
    #[cfg(test)]
    pub fn disk() -> Self {
        Self {
            arg: PathBuf::new(),
            entry: PathBuf::new(),
            zip: None,
        }
    }

    /// 入口のファイル。
    pub fn entry(&self) -> &Path {
        &self.entry
    }

    /// 入口がシェーダーか（拡張子で決める）。
    pub fn is_shader(&self) -> bool {
        crate::shader::is_shader_path(&self.entry)
    }

    /// 読み直しを始める。zip ファイルは読み直す（入口の場所は起動時のまま）。
    pub fn reload(&mut self) {
        if self.zip.is_some() {
            self.zip = Some(Zip::read(&self.arg));
        }
    }

    /// 入口のファイルを読むときに見張るファイル（zip ファイルを渡したときは zip ファイル）。
    pub fn watched_entry(&self) -> PathBuf {
        self.watch_paths(vec![self.entry.clone()]).remove(0)
    }

    /// 見張るファイル。zip ファイルを渡したときは、zip ファイルだけを見張る。
    pub fn watch_paths(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        match self.zip {
            Some(_) => vec![self.arg.clone()],
            None => paths,
        }
    }

    /// 指定に書かれたままのパスを、この壁紙で使えるか確かめる。zip の中では `~/` と絶対パスを使えない。
    pub fn check_path(&self, raw: &str) -> Result<(), String> {
        if self.zip.is_some() && (raw.starts_with("~/") || Path::new(raw).is_absolute()) {
            return Err(format!(
                "{raw}: absolute paths and ~/ cannot be used in a zip file"
            ));
        }
        Ok(())
    }

    /// ファイルを読む。エラーは `<パス>: <内容>` の形。
    pub fn read(&self, path: &Path) -> Result<Vec<u8>, String> {
        let fail = |e: String| format!("{}: {e}", path.display());
        let Some(zip) = &self.zip else {
            return std::fs::read(path).map_err(|e| fail(e.to_string()));
        };
        let archive = zip.archive.as_ref().map_err(Clone::clone)?;
        let name = self
            .zip_name(path)
            .ok_or_else(|| fail("outside the zip file".into()))?;
        let entry = archive
            .find(&name)
            .filter(|e| !e.name.ends_with('/'))
            .ok_or_else(|| fail("not found in the zip file".into()))?;
        {
            let mut extracted = zip.extracted.lock().unwrap_or_else(|e| e.into_inner());
            if !extracted.contains_key(&name) {
                let total = extracted.values().sum::<u64>() + entry.size;
                if total > ZIP_EXTRACT_LIMIT {
                    return Err(format!(
                        "{}: the wallpaper's files are {} MiB when extracted (max {} MiB, at {name}). \
                         Extract the zip and pass the directory instead",
                        self.arg.display(),
                        total.div_ceil(1024 * 1024),
                        ZIP_EXTRACT_LIMIT / (1024 * 1024)
                    ));
                }
                extracted.insert(name, entry.size);
            }
        }
        archive.extract(entry).map_err(fail)
    }

    pub fn read_to_string(&self, path: &Path) -> Result<String, String> {
        let bytes = self.read(path)?;
        String::from_utf8(bytes)
            .map_err(|_| format!("{}: stream did not contain valid UTF-8", path.display()))
    }

    /// `<zip のパス>/<zip の中のパス>` の、zip の中のパス。`..` を解決し、zip の外に出れば None。
    fn zip_name(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.arg).ok()?;
        let mut parts: Vec<&str> = Vec::new();
        for c in rel.components() {
            match c {
                Component::Normal(s) => parts.push(s.to_str()?),
                Component::CurDir => {}
                Component::ParentDir => {
                    parts.pop()?;
                }
                Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        (!parts.is_empty()).then(|| parts.join("/"))
    }
}

/// 起動時に壁紙のファイルを読めるか確かめる（#37）。
/// 起動時に読めないのはパスの打ち間違いがほとんどなので、Wayland に接続する前に終了する。
/// 実行中に消えた場合は保存の途中とみなして待つ（`watcher` を参照）。
fn check_readable(path: &Path) -> Result<(), String> {
    // ディレクトリも File::open では開けてしまうので、読んで確かめる。
    std::fs::read(path)
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::zipfile::test_zip::{File, build};

    /// テストごとに別の一時ディレクトリにファイルを書く。
    pub struct Dir(pub PathBuf);

    impl Dir {
        pub fn new(name: &str) -> Self {
            let d =
                std::env::temp_dir().join(format!("kagegami-source-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }

        pub fn write(&self, name: &str, data: &[u8]) -> PathBuf {
            let p = self.0.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, data).unwrap();
            p
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn file_is_its_own_entry() {
        let d = Dir::new("file");
        let p = d.write("a.glsl", b"x");
        let s = Source::open(&p).unwrap();
        assert_eq!(s.entry(), p);
        assert!(s.is_shader());
        assert_eq!(s.watch_paths(vec![p.clone()]), vec![p]);
        // 読めないファイルは、起動時にパスと理由を出して終了する（#37）。
        let missing = d.0.join("missing.lua");
        let e = Source::open(&missing).err().unwrap();
        assert!(e.contains(&missing.display().to_string()), "{e}");
        assert!(e.contains("No such file or directory"), "{e}");
    }

    #[test]
    fn directory_entry_is_the_only_main_file() {
        let d = Dir::new("dir");
        d.write("w/sim.glsl", b"x");
        let e = Source::open(&d.0.join("w")).err().unwrap();
        assert!(
            e.contains("no entry file (main.glsl, main.frag, main.lua)"),
            "{e}"
        );
        let main = d.write("w/main.lua", b"-- lua");
        let s = Source::open(&d.0.join("w")).unwrap();
        assert_eq!(s.entry(), main);
        assert!(!s.is_shader());
        // 末尾の `/` があっても同じ。
        let s = Source::open(Path::new(&format!("{}/", d.0.join("w").display()))).unwrap();
        assert_eq!(std::fs::read(s.entry()).unwrap(), b"-- lua");
        d.write("w/main.glsl", b"x");
        let e = Source::open(&d.0.join("w")).err().unwrap();
        assert!(e.contains("more than one entry file"), "{e}");
    }

    fn zip_source(d: &Dir, files: &[File]) -> Result<Source, String> {
        let p = d.write("w.zip", &build(files));
        Source::open(&p)
    }

    #[test]
    fn zip_entry_is_at_the_top_or_in_the_only_directory() {
        let d = Dir::new("zip-entry");
        let s = zip_source(&d, &[File::new("main.glsl", b"top")]).unwrap();
        assert_eq!(s.entry(), d.0.join("w.zip/main.glsl"));
        assert_eq!(s.read(s.entry()).unwrap(), b"top");

        let s = zip_source(
            &d,
            &[
                File::new("ink/", b"").stored(),
                File::new("ink/main.glsl", b"inner"),
                File::new("ink/sim.glsl", b"sim"),
            ],
        )
        .unwrap();
        assert_eq!(s.entry(), d.0.join("w.zip/ink/main.glsl"));
        // ディレクトリのエントリーがない zip でも同じ。
        let s = zip_source(&d, &[File::new("ink/main.lua", b"lua")]).unwrap();
        assert_eq!(s.entry(), d.0.join("w.zip/ink/main.lua"));

        for (files, want) in [
            (
                vec![File::new("a/main.glsl", b""), File::new("b/x", b"")],
                "no entry file",
            ),
            (
                vec![File::new("a/main.glsl", b""), File::new("x.png", b"")],
                "no entry file",
            ),
            (
                vec![File::new("main.glsl", b""), File::new("main.lua", b"")],
                "more than one entry file (main.glsl, main.lua)",
            ),
            (
                vec![File::new("a/main.glsl", b""), File::new("a/main.frag", b"")],
                "w.zip: a/: more than one entry file",
            ),
        ] {
            let e = zip_source(&d, &files).err().unwrap();
            assert!(e.contains(want), "{e}");
        }
        d.write("bad.zip", b"not a zip");
        assert!(Source::open(&d.0.join("bad.zip")).is_err());
    }

    #[test]
    fn zip_reads_relative_paths_and_nothing_outside() {
        let d = Dir::new("zip-paths");
        d.write("outside.png", b"secret");
        let s = zip_source(
            &d,
            &[
                File::new("main.glsl", b"m"),
                File::new("lib/a.glsl", b"a"),
                File::new("lib/sub/b.glsl", b"b"),
            ],
        )
        .unwrap();
        let base = d.0.join("w.zip/lib/sub");
        assert_eq!(s.read(&base.join("b.glsl")).unwrap(), b"b");
        assert_eq!(s.read(&base.join("./../sub/b.glsl")).unwrap(), b"b");
        assert_eq!(s.read(&base.join("../a.glsl")).unwrap(), b"a");
        assert_eq!(s.read(&base.join("../../main.glsl")).unwrap(), b"m");
        let e = s.read(&base.join("../../../outside.png")).unwrap_err();
        assert!(e.ends_with("outside.png: outside the zip file"), "{e}");
        assert!(
            s.read(&d.0.join("outside.png"))
                .unwrap_err()
                .contains("outside the zip file")
        );
        assert!(
            s.read(&d.0.join("w.zip/lib"))
                .unwrap_err()
                .contains("not found in the zip file")
        );
        assert!(
            s.read(&base.join("missing.glsl"))
                .unwrap_err()
                .contains("not found")
        );
        assert!(s.check_path("~/a.png").is_err());
        assert!(s.check_path("/abs/a.png").is_err());
        assert!(s.check_path("lib/a.glsl").is_ok());
        // ディスクから読むときは、どちらも使える。
        let disk = Source::open(&d.0.join("outside.png")).unwrap();
        assert!(disk.check_path("~/a.png").is_ok() && disk.check_path("/abs/a.png").is_ok());
    }

    #[test]
    fn zip_limits_the_total_extracted_size_before_extracting() {
        let d = Dir::new("zip-limit");
        let mib = 1024 * 1024;
        // 中身は小さいが、中央ディレクトリには大きなサイズが書いてある（展開する前に止まることを確かめる）。
        let big = |name, size| File {
            size: Some(size),
            ..File::new(name, b"x")
        };
        let s = zip_source(
            &d,
            &[
                File::new("main.glsl", b"m"),
                big("a.png", 200 * mib),
                big("b.png", 100 * mib),
                big("unused.png", 4000 * mib),
            ],
        )
        .unwrap();
        let p = |n: &str| d.0.join("w.zip").join(n);
        s.read(&p("main.glsl")).unwrap();
        // a.png は上限の内なので展開まで進み、書かれたサイズと合わないエラーになる。
        assert!(
            s.read(&p("a.png"))
                .unwrap_err()
                .contains("size does not match")
        );
        // 同じファイルを 2 回読んでも 1 回と数える（出力ごとに読むため）。
        assert!(!s.read(&p("a.png")).unwrap_err().contains("max 256 MiB"));
        let e = s.read(&p("b.png")).unwrap_err();
        assert!(
            e.contains("w.zip: the wallpaper's files are 301 MiB when extracted (max 256 MiB, at b.png). Extract the zip and pass the directory instead"),
            "{e}"
        );
    }

    #[test]
    fn zip_is_read_again_on_reload_and_only_the_zip_is_watched() {
        let d = Dir::new("zip-reload");
        let mut s = zip_source(&d, &[File::new("main.glsl", b"v1")]).unwrap();
        assert_eq!(
            s.watch_paths(vec![s.entry().to_path_buf()]),
            vec![d.0.join("w.zip")]
        );
        d.write("w.zip", &build(&[File::new("main.glsl", b"v2")]));
        // 中身は展開するときに zip ファイルから読む。読み直す前に置き換えられた zip は、前の中央
        // ディレクトリと合わずにエラーになる（見張っているので、すぐに読み直す）。
        assert!(
            s.read(s.entry())
                .unwrap_err()
                .contains("CRC does not match")
        );
        s.reload();
        assert_eq!(s.read(s.entry()).unwrap(), b"v2");
        // 保存の途中で壊れていても、入口の場所は変えずにエラーを返す。
        d.write("w.zip", b"broken");
        s.reload();
        assert!(
            s.read(s.entry())
                .unwrap_err()
                .contains("w.zip: not a zip file")
        );
    }
}

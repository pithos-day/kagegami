//! zip ファイルの読み込み（#60）。中央ディレクトリを読み、stored と deflate のエントリーを展開する。
//!
//! ZIP64・暗号化・複数のディスクに分けた zip と、stored と deflate 以外の圧縮方式は読まない。
//! 展開は `miniz_oxide`、CRC は `crc32fast` で確かめる（どちらも `png` が使っているもの）。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// zip ファイルの中央ディレクトリ。エントリーの中身は、展開するときにその範囲だけをファイルから読む
/// （壁紙が使わない大きなファイルが入っていても、メモリーに載せない）。
pub struct Archive {
    path: PathBuf,
    entries: Vec<Entry>,
}

/// 中央ディレクトリの 1 つのエントリー。
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// zip の中のパス（区切りは `/`。ディレクトリは `/` で終わる）。
    pub name: String,
    /// 中央ディレクトリに書かれた、展開した後のサイズ。
    pub size: u64,
    method: u16,
    flags: u16,
    crc: u32,
    compressed: u64,
    local_header: u64,
}

const EOCD_SIG: u32 = 0x0605_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
const EOCD_LEN: usize = 22;
const LOCAL_LEN: usize = 30;

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        d.get(o..o.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        d.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn truncated() -> String {
    "broken zip file (truncated)".to_string()
}

/// `f` の `offset` から `len` バイトを読む。ファイルの末尾で足りなければ truncated のエラー。
fn read_at(f: &mut File, offset: u64, len: u64) -> Result<Vec<u8>, String> {
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    f.take(len)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    if buf.len() as u64 != len {
        return Err(truncated());
    }
    Ok(buf)
}

impl Archive {
    /// zip ファイルの中央ディレクトリを読む。
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut f = File::open(path).map_err(|e| e.to_string())?;
        let len = f.metadata().map_err(|e| e.to_string())?.len();
        // 末尾のコメントは最大 65535 バイト。長さがファイルの末尾と合う署名を、後ろから探す。
        let tail_len = len.min((EOCD_LEN + 0xffff) as u64);
        let tail = read_at(&mut f, len - tail_len, tail_len)?;
        let eocd = (0..=tail.len().saturating_sub(EOCD_LEN))
            .rev()
            .find(|&i| {
                u32_at(&tail, i) == Some(EOCD_SIG)
                    && u16_at(&tail, i + 20).map(|n| i + EOCD_LEN + n as usize) == Some(tail.len())
            })
            .ok_or("not a zip file (end of central directory not found)")?;
        let field16 = |o| u16_at(&tail, eocd + o).ok_or_else(truncated);
        let field32 = |o| u32_at(&tail, eocd + o).ok_or_else(truncated);
        let (disk, cd_disk, count) = (field16(4)?, field16(6)?, field16(10)?);
        let (cd_size, cd_offset) = (field32(12)?, field32(16)?);
        if count == 0xffff || cd_size == 0xffff_ffff || cd_offset == 0xffff_ffff {
            return Err("ZIP64 is not supported".into());
        }
        if disk != 0 || cd_disk != 0 {
            return Err("multi-disk zip files are not supported".into());
        }
        let eocd_offset = len - tail_len + eocd as u64;
        if cd_offset as u64 + cd_size as u64 > eocd_offset {
            return Err("broken zip file (bad central directory)".into());
        }
        let cd = read_at(&mut f, cd_offset as u64, cd_size as u64)?;

        let mut entries = Vec::with_capacity(count as usize);
        let mut p = 0;
        for _ in 0..count {
            if u32_at(&cd, p) != Some(CENTRAL_SIG) {
                return Err("broken zip file (bad central directory)".into());
            }
            let field16 = |o| u16_at(&cd, p + o).ok_or_else(truncated);
            let field32 = |o| u32_at(&cd, p + o).ok_or_else(truncated);
            let flags = field16(8)?;
            let method = field16(10)?;
            let crc = field32(16)?;
            let compressed = field32(20)?;
            let size = field32(24)?;
            let name_len = field16(28)? as usize;
            let extra_len = field16(30)? as usize;
            let comment_len = field16(32)? as usize;
            let local_header = field32(42)?;
            if compressed == 0xffff_ffff || size == 0xffff_ffff || local_header == 0xffff_ffff {
                return Err("ZIP64 is not supported".into());
            }
            let name = cd.get(p + 46..p + 46 + name_len).ok_or_else(truncated)?;
            entries.push(Entry {
                name: String::from_utf8_lossy(name).into_owned(),
                size: size as u64,
                method,
                flags,
                crc,
                compressed: compressed as u64,
                local_header: local_header as u64,
            });
            p += 46 + name_len + extra_len + comment_len;
        }
        Ok(Self {
            path: path.to_path_buf(),
            entries,
        })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// 名前が `name` のエントリー。同じ名前が 2 つ以上あれば、中央ディレクトリで先のもの。
    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// エントリーを展開する。中央ディレクトリに書かれたサイズを超えて展開されるもの、
    /// サイズか CRC が合わないものはエラーにする。
    pub fn extract(&self, e: &Entry) -> Result<Vec<u8>, String> {
        if e.flags & 1 != 0 {
            return Err("encrypted zip entries are not supported".into());
        }
        // 中央ディレクトリを読んだ後に zip ファイルが置き換えられていても、ローカルヘッダー・
        // サイズ・CRC のどれかが合わずにエラーになる（読み直しで新しい中央ディレクトリを読む）。
        let mut f = File::open(&self.path).map_err(|e| e.to_string())?;
        let lh = read_at(&mut f, e.local_header, LOCAL_LEN as u64)?;
        if u32_at(&lh, 0) != Some(LOCAL_SIG) {
            return Err("broken zip file (bad local header)".into());
        }
        // ローカルヘッダーの名前と extra の長さは、中央ディレクトリと違うことがある。
        let name_len = u16_at(&lh, 26).ok_or_else(truncated)? as u64;
        let extra_len = u16_at(&lh, 28).ok_or_else(truncated)? as u64;
        let start = e.local_header + LOCAL_LEN as u64 + name_len + extra_len;
        let raw = read_at(&mut f, start, e.compressed)?;
        let size = e.size as usize;
        let out = match e.method {
            0 => raw,
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(&raw, size).map_err(|err| {
                match err.status {
                    miniz_oxide::inflate::TINFLStatus::HasMoreOutput => {
                        format!("larger than the size written in the zip file ({size} bytes)")
                    }
                    s => format!("broken deflate data ({s:?})"),
                }
            })?,
            m => {
                return Err(format!(
                    "compression method {m} is not supported (only stored and deflate)"
                ));
            }
        };
        if out.len() != size {
            return Err(format!(
                "size does not match the zip file ({} bytes, expected {size})",
                out.len()
            ));
        }
        if crc32fast::hash(&out) != e.crc {
            return Err("CRC does not match (broken zip file)".into());
        }
        Ok(out)
    }
}

/// テスト用に zip ファイルを作る。
#[cfg(test)]
pub mod test_zip {
    /// 1 つのエントリー。`size`・`crc` を `Some` にすると、中央ディレクトリにその値を書く（偽りの zip を作る）。
    pub struct File<'a> {
        pub name: &'a str,
        pub data: &'a [u8],
        pub deflate: bool,
        pub size: Option<u32>,
        pub crc: Option<u32>,
        pub method: Option<u16>,
        pub flags: u16,
    }

    impl<'a> File<'a> {
        pub fn new(name: &'a str, data: &'a [u8]) -> Self {
            Self {
                name,
                data,
                deflate: true,
                size: None,
                crc: None,
                method: None,
                flags: 0,
            }
        }

        pub fn stored(mut self) -> Self {
            self.deflate = false;
            self
        }
    }

    pub fn build(files: &[File]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for f in files {
            let body = if f.deflate {
                miniz_oxide::deflate::compress_to_vec(f.data, 6)
            } else {
                f.data.to_vec()
            };
            let method = f.method.unwrap_or(if f.deflate { 8 } else { 0 });
            let crc = f.crc.unwrap_or(crc32fast::hash(f.data));
            let size = f.size.unwrap_or(f.data.len() as u32);
            let offset = out.len() as u32;
            let common = |v: &mut Vec<u8>| {
                v.extend_from_slice(&20u16.to_le_bytes()); // version needed
                v.extend_from_slice(&f.flags.to_le_bytes());
                v.extend_from_slice(&method.to_le_bytes());
                v.extend_from_slice(&[0; 4]); // time, date
                v.extend_from_slice(&crc.to_le_bytes());
                v.extend_from_slice(&(body.len() as u32).to_le_bytes());
                v.extend_from_slice(&size.to_le_bytes());
                v.extend_from_slice(&(f.name.len() as u16).to_le_bytes());
                v.extend_from_slice(&0u16.to_le_bytes()); // extra
            };
            out.extend_from_slice(&super::LOCAL_SIG.to_le_bytes());
            common(&mut out);
            out.extend_from_slice(f.name.as_bytes());
            out.extend_from_slice(&body);

            central.extend_from_slice(&super::CENTRAL_SIG.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            common(&mut central);
            central.extend_from_slice(&[0; 6]); // comment, disk, internal attr
            central.extend_from_slice(&[0; 4]); // external attr
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(f.name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&super::EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&[0; 4]); // disk numbers
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out
    }
}

#[cfg(test)]
mod tests {
    use super::test_zip::{File, build};
    use super::*;

    /// テストの zip を一時ファイルに書いて開く。ファイルは `Tmp` を捨てると消える。
    struct Tmp(PathBuf);

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn parse(data: Vec<u8>) -> Result<(Archive, Tmp), String> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p =
            std::env::temp_dir().join(format!("kagegami-zipfile-{}-{n}.zip", std::process::id()));
        std::fs::write(&p, data).unwrap();
        let tmp = Tmp(p);
        Archive::open(&tmp.0).map(|a| (a, tmp))
    }

    fn extract(files: &[File], name: &str) -> Result<Vec<u8>, String> {
        let (a, _tmp) = parse(build(files))?;
        let e = a.find(name).ok_or("not found")?.clone();
        a.extract(&e)
    }

    #[test]
    fn extracts_stored_and_deflate_entries() {
        let text = b"void mainImage() {}\n".repeat(50);
        let (a, _tmp) = parse(build(&[
            File::new("ink/", b"").stored(),
            File::new("ink/main.glsl", &text),
            File::new("ink/a.glsl", b"stored").stored(),
            File::new("empty", b""),
        ]))
        .unwrap();
        let names: Vec<_> = a.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["ink/", "ink/main.glsl", "ink/a.glsl", "empty"]);
        for (name, want) in [
            ("ink/main.glsl", &text[..]),
            ("ink/a.glsl", b"stored"),
            ("empty", b""),
        ] {
            let e = a.find(name).unwrap();
            assert_eq!(e.size, want.len() as u64);
            assert_eq!(a.extract(e).unwrap(), want, "{name}");
        }
    }

    #[test]
    fn finds_the_end_record_after_a_comment() {
        let mut z = build(&[File::new("a", b"x")]);
        let n = z.len();
        z[n - 2..].copy_from_slice(&5u16.to_le_bytes());
        z.extend_from_slice(b"hello");
        let (a, _tmp) = parse(z).unwrap();
        assert_eq!(a.extract(a.find("a").unwrap()).unwrap(), b"x");
    }

    #[test]
    fn rejects_data_larger_than_the_written_size() {
        let data = vec![0u8; 100_000];
        let f = File {
            size: Some(10),
            ..File::new("a", &data)
        };
        let e = extract(&[f], "a").unwrap_err();
        assert!(e.contains("larger than the size written"), "{e}");
    }

    #[test]
    fn rejects_wrong_size_crc_and_methods() {
        let short = File {
            size: Some(10),
            ..File::new("a", b"abc").stored()
        };
        assert!(
            extract(&[short], "a")
                .unwrap_err()
                .contains("size does not match")
        );
        let bad_crc = File {
            crc: Some(1),
            ..File::new("a", b"abc")
        };
        assert!(extract(&[bad_crc], "a").unwrap_err().contains("CRC"));
        let bzip2 = File {
            method: Some(12),
            ..File::new("a", b"abc").stored()
        };
        assert!(extract(&[bzip2], "a").unwrap_err().contains("method 12"));
    }

    #[test]
    fn rejects_encrypted_zip64_and_multi_disk_files() {
        let encrypted = File {
            flags: 1,
            ..File::new("a", b"abc")
        };
        assert!(
            extract(&[encrypted], "a")
                .unwrap_err()
                .contains("encrypted")
        );
        let z = build(&[File::new("a", b"abc")]);
        let n = z.len();
        // EOCD の中央ディレクトリの位置が 0xffffffff なら ZIP64。
        let mut zip64 = z.clone();
        zip64[n - 6..n - 2].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(zip64).err().unwrap().contains("ZIP64"));
        // 中央ディレクトリのエントリーのサイズが 0xffffffff でも ZIP64。
        let size64 = File {
            size: Some(u32::MAX),
            ..File::new("a", b"abc")
        };
        assert!(parse(build(&[size64])).err().unwrap().contains("ZIP64"));
        let mut disk = z.clone();
        disk[n - 18..n - 16].copy_from_slice(&1u16.to_le_bytes());
        assert!(parse(disk).err().unwrap().contains("multi-disk"));
    }

    #[test]
    fn rejects_broken_files() {
        assert!(parse(b"not a zip".to_vec()).is_err());
        assert!(parse(Vec::new()).is_err());
        let z = build(&[File::new("a", b"abc")]);
        // 中央ディレクトリのエントリーの数が、実際より多いもの。
        let mut more = z.clone();
        let n = more.len();
        more[n - 12..n - 10].copy_from_slice(&2u16.to_le_bytes());
        assert!(parse(more).err().unwrap().contains("bad central directory"));
        // 中央ディレクトリの場所が、ファイルの外を指すもの。
        let mut outside = z.clone();
        outside[n - 6..n - 2].copy_from_slice(&0x00ff_ffffu32.to_le_bytes());
        assert!(parse(outside).is_err());
        // ローカルヘッダーを壊したもの。
        let mut broken = z.clone();
        broken[0] = 0;
        let (a, _tmp) = parse(broken).unwrap();
        assert!(
            a.extract(a.find("a").unwrap())
                .unwrap_err()
                .contains("local header")
        );
    }
}

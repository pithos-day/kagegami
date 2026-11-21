//! 標準エラー出力への書き込み。
//!
//! `--perf` で TTY に出す負荷の表示は、最後の数行をカーソル移動で書き換え続ける。
//! ほかのメッセージ（エラー、スクリプトの `print` など）がその途中に書かれると、
//! 書き換える行がずれて表示が壊れるので、標準エラー出力にはこのモジュールを通して書く。
//! メッセージを書くときは負荷の表示を消してから書き、負荷の表示は次の更新で一番下に描き直す。

use std::fmt::Display;
use std::io::Write;
use std::sync::Mutex;

/// 今描いている負荷の表示の行数。描いていなければ 0。
static DRAWN_LINES: Mutex<usize> = Mutex::new(0);

/// `n` 行の表示を消すエスケープシーケンス（`n` 行上の行頭へ移り、そこから下を消す）。
fn erase(n: usize) -> String {
    if n == 0 {
        String::new()
    } else {
        format!("\x1b[{n}F\x1b[J")
    }
}

/// 1 行のメッセージを書く。負荷の表示を描いていれば、先に消す。
pub fn line(msg: impl Display) {
    let mut drawn = DRAWN_LINES.lock().unwrap_or_else(|e| e.into_inner());
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{}{msg}", erase(*drawn));
    *drawn = 0;
}

/// 負荷の表示を描き直す。前に描いた表示は消す。
/// 端末の幅を超えた行が折り返されると消す行の数がずれるので、描く間は自動の折り返しを止める。
pub fn redraw_block(lines: &[String]) {
    let mut drawn = DRAWN_LINES.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = erase(*drawn);
    out.push_str("\x1b[?7l");
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str("\x1b[?7h");
    let _ = std::io::stderr().lock().write_all(out.as_bytes());
    *drawn = lines.len();
}

/// `eprintln!` の代わり。負荷の表示と混ざらないように書く。
macro_rules! elog {
    ($($arg:tt)*) => {
        $crate::stderr::line(format_args!($($arg)*))
    };
}
pub(crate) use elog;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erase_moves_up_and_clears() {
        assert_eq!(erase(0), "");
        assert_eq!(erase(3), "\x1b[3F\x1b[J");
    }
}

//! `--perf`: kagegami プロセスの GPU 使用率を、DRM の fdinfo から測る（#45）。
//!
//! DRM のドライバーは、開いている fd ごとに `/proc/<pid>/fdinfo/<fd>` へ
//! エンジンごとの使用時間（`drm-engine-<名前>: <値> ns`）を出す（カーネルの DRM usage stats）。
//! 1 秒ごとの増え方を経過時間で割って、エンジンごとの使用率にする。
//!
//! - 同じクライアント（`drm-client-id`）が複数の fd に出るので、重複を除いて数える
//! - 単位が `ns` の値だけを読む。サイクル数（`drm-cycles-*`）で出すドライバー（xe など）と、
//!   fdinfo に使用量を出さないドライバー（NVIDIA のプロプライエタリドライバーなど）では何も取れない
//! - 検証したのは amdgpu だけ

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

/// 1 つの DRM クライアントの、エンジンごとの値。
#[derive(Debug, Default, Clone, PartialEq)]
struct Client {
    id: u64,
    /// エンジンの名前 → 使用時間（ns）。
    busy_ns: BTreeMap<String, u64>,
    /// エンジンの名前 → 容量（同じ種類のエンジンの数）。書かれていなければ 1。
    capacity: BTreeMap<String, u64>,
}

/// fdinfo の 1 ファイルを読む。DRM の fd でなければ None。
fn parse_fdinfo(text: &str) -> Option<Client> {
    let mut client = Client::default();
    let mut has_id = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if key == "drm-client-id" {
            client.id = value.parse().ok()?;
            has_id = true;
        } else if let Some(name) = key.strip_prefix("drm-engine-capacity-") {
            if let Ok(n) = value.parse() {
                client.capacity.insert(name.to_string(), n);
            }
        } else if let Some(name) = key.strip_prefix("drm-engine-") {
            let Some(ns) = value.strip_suffix(" ns") else {
                continue;
            };
            if let Ok(ns) = ns.trim().parse() {
                client.busy_ns.insert(name.to_string(), ns);
            }
        }
    }
    has_id.then_some(client)
}

/// エンジンごとの使用時間の合計。
#[derive(Debug, Default, Clone, PartialEq)]
struct Totals {
    /// エンジンの名前 → (使用時間の合計（ns）, 容量)。
    engines: BTreeMap<String, (u64, u64)>,
}

/// クライアントの重複を除き、エンジンごとに使用時間を足す。
fn total(clients: impl IntoIterator<Item = Client>) -> Totals {
    let mut by_id: HashMap<u64, Client> = HashMap::new();
    for c in clients {
        by_id.entry(c.id).or_insert(c);
    }
    let mut t = Totals::default();
    for c in by_id.into_values() {
        for (name, ns) in c.busy_ns {
            let cap = c.capacity.get(&name).copied().unwrap_or(1).max(1);
            let e = t.engines.entry(name).or_insert((0, cap));
            e.0 += ns;
            e.1 = e.1.max(cap);
        }
    }
    t
}

/// 前の合計から今の合計までの使用率（%）。エンジンの名前の順で、増えていないエンジンは除く。
fn usage(prev: &Totals, now: &Totals, elapsed_ns: f64) -> Vec<(String, f64)> {
    if elapsed_ns <= 0.0 {
        return Vec::new();
    }
    now.engines
        .iter()
        .filter_map(|(name, &(ns, cap))| {
            let before = prev.engines.get(name).map_or(0, |e| e.0);
            let delta = ns.saturating_sub(before);
            (delta > 0).then(|| (name.clone(), delta as f64 / elapsed_ns / cap as f64 * 100.0))
        })
        .collect()
}

/// `/proc/self/fdinfo` を読んで、DRM クライアントを集める。
fn read_clients() -> Vec<Client> {
    let Ok(dir) = std::fs::read_dir("/proc/self/fdinfo") else {
        return Vec::new();
    };
    dir.filter_map(|e| std::fs::read_to_string(e.ok()?.path()).ok())
        .filter_map(|t| parse_fdinfo(&t))
        .collect()
}

/// kagegami プロセスの GPU 使用率を測る。
pub struct GpuUsageMeter {
    last: Option<(Totals, Instant)>,
}

impl GpuUsageMeter {
    pub fn new() -> Self {
        Self { last: None }
    }

    /// 前に測ってからの、エンジンごとの使用率（%）。
    /// fdinfo から使用時間を 1 つも取れない環境では None（表示しない）。
    /// 最初の 1 回は比べる値がないので None。
    pub fn sample(&mut self, now: Instant) -> Option<Vec<(String, f64)>> {
        let totals = total(read_clients());
        if totals.engines.is_empty() {
            self.last = None;
            return None;
        }
        let r = self
            .last
            .as_ref()
            .map(|(prev, at)| usage(prev, &totals, now.duration_since(*at).as_nanos() as f64));
        self.last = Some((totals, now));
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手元の amdgpu で読めた fdinfo（9b6f742 の kagegami）と同じ形。
    const AMDGPU: &str = "pos:\t0\nflags:\t02100002\nmnt_id:\t26\nino:\t1047\n\
        drm-driver:\tamdgpu\ndrm-client-id:\t361\ndrm-pdev:\t0000:03:00.0\n\
        drm-total-gtt:\t2048 KiB\ndrm-engine-gfx:\t536152 ns\n";

    #[test]
    fn parses_amdgpu_fdinfo() {
        let c = parse_fdinfo(AMDGPU).unwrap();
        assert_eq!(c.id, 361);
        assert_eq!(c.busy_ns, BTreeMap::from([("gfx".to_string(), 536152)]));
    }

    #[test]
    fn ignores_non_drm_and_non_ns_values() {
        // DRM でない fd。
        assert_eq!(parse_fdinfo("pos:\t0\nflags:\t02\nmnt_id:\t15\n"), None);
        // 容量は使用時間として数えない。サイクル数（xe など）は読まない。
        let c = parse_fdinfo(
            "drm-driver:\ti915\ndrm-client-id:\t7\ndrm-engine-render:\t100 ns\n\
             drm-engine-capacity-video:\t2\ndrm-engine-video:\t50 ns\n\
             drm-cycles-rcs:\t999\ndrm-total-cycles-rcs:\t9999\n",
        )
        .unwrap();
        assert_eq!(
            c.busy_ns,
            BTreeMap::from([("render".to_string(), 100), ("video".to_string(), 50)])
        );
        assert_eq!(c.capacity, BTreeMap::from([("video".to_string(), 2)]));
        let xe = parse_fdinfo("drm-driver:\txe\ndrm-client-id:\t3\ndrm-cycles-rcs:\t10\n").unwrap();
        assert!(total([xe]).engines.is_empty());
    }

    #[test]
    fn counts_each_client_once() {
        // 同じクライアントが 3 つの fd に出ても、1 回だけ数える。
        let c = parse_fdinfo(AMDGPU).unwrap();
        let t = total([c.clone(), c.clone(), c]);
        assert_eq!(t.engines["gfx"], (536152, 1));
        // 別のクライアントは足す。
        let mut other = parse_fdinfo(AMDGPU).unwrap();
        other.id = 362;
        let t = total([parse_fdinfo(AMDGPU).unwrap(), other]);
        assert_eq!(t.engines["gfx"], (536152 * 2, 1));
    }

    #[test]
    fn usage_is_delta_over_elapsed_and_skips_idle_engines() {
        let t = |gfx, dma, video| Totals {
            engines: BTreeMap::from([
                ("gfx".to_string(), (gfx, 1)),
                ("dma".to_string(), (dma, 1)),
                ("video".to_string(), (video, 2)),
            ]),
        };
        let prev = t(1_000_000, 500, 0);
        // 1 秒で gfx が 80 ms、video（容量 2）が 100 ms 使われ、dma は増えていない。
        let now = t(81_000_000, 500, 100_000_000);
        let u = usage(&prev, &now, 1e9);
        assert_eq!(u, [("gfx".to_string(), 8.0), ("video".to_string(), 5.0)]);
        assert!(usage(&prev, &now, 0.0).is_empty());
    }
}

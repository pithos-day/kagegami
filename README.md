# kagegami

Lua で書けるライブ壁紙。Wayland（wlr-layer-shell）の背景レイヤーに描画する。

点や線などの図形を「1 フレームに一度だけ計算して、図形として GPU に送る」ので、
全ピクセルで同じ計算を繰り返すシェーダー方式の壁紙より GPU の負荷が小さい。

## ビルド

必要なもの: Rust、`wayland-client` / `wayland-egl` / EGL / GLES2 の開発ファイル、C コンパイラー（LuaJIT を同梱してビルドするため）。

```sh
cargo build --release
cp target/release/kagegami ~/.local/bin/
```

## 使い方

```sh
kagegami [--fps N] [--layer background|bottom|top|overlay] <output-name|*> <wallpaper.lua>
```

- `--fps`: フレームレートの上限（既定 30）
- `--layer`: 描画するレイヤー（既定 `background`）
- `<output-name>`: `niri msg outputs` などで出る出力名（`DP-1` など）。`*` で全出力
- 壁紙のファイルを保存すると、自動で読み直す

例:

```sh
kagegami --fps 30 '*' examples/geometry.lua
```

### 自動起動

スクリプトが戻ってこなくなった場合（後述）、kagegami は終了コード 3 で終了する。
niri の `spawn-at-startup` は終了したプロセスを再起動しないので、終了コードが 3 のときだけ起動し直すループで包むとよい。
ほかの理由（Wayland への接続失敗など）で終了したときは、ループを抜ける。

`~/.config/niri/config.kdl`:

```kdl
spawn-at-startup "sh" "-c" "while :; do kagegami --fps 30 '*' ~/.config/kagegami/geometry.lua; [ $? -eq 3 ] || break; sleep 2; done"
```

systemd を使っている環境なら、ユーザーユニットに `Restart=on-failure` を付けて動かしてもよい。

## 壁紙の書き方

壁紙は LuaJIT（Lua 5.1 の文法）で書く。出力（モニター）ごとに独立した Lua の実行環境が作られるので、
モニターごとに乱数も状態も別になる。

```lua
local t = 0

function update(dt)
  t = t + dt
end

function draw()
  clear(0.04, 0.04, 0.04)
  color(1, 1, 1, 0.8)
  circle(screen.w / 2 + math.cos(t) * 200, screen.h / 2, 20)
end
```

### コールバック

どれも省略できる。

| 関数 | 呼ばれるタイミング |
|---|---|
| `load()` | 読み込み直後に 1 回 |
| `update(dt)` | 毎フレーム。`dt` は前のフレームからの秒数（最大 0.25） |
| `draw()` | 毎フレーム、`update` の後 |
| `mousepressed(x, y, button)` | ボタンを押したとき。`button` は `"left"` / `"right"` / `"middle"` |
| `mousereleased(x, y, button)` | ボタンを離したとき |

### 描画関数

座標はピクセル単位で、左上が原点。

| 関数 | 説明 |
|---|---|
| `clear(r, g, b)` | 画面を塗りつぶす。呼ばなかったフレームは黒になる |
| `color(r, g, b, a)` | 以降の図形の色。各値は 0〜1、`a` は省略すると 1 |
| `blend(mode)` | `"alpha"`（既定、普通の重ね塗り）か `"add"`（加算合成） |
| `line(x1, y1, x2, y2, width)` | 端が丸い線。`width` は省略すると 1 |
| `circle(x, y, r)` | 塗りつぶした円 |
| `rect(x, y, w, h)` | 塗りつぶした矩形 |

### 渡される情報

| 変数 | 内容 |
|---|---|
| `screen.w`, `screen.h` | 画面のピクセルサイズ |
| `screen.output` | 出力名 |
| `mouse.x`, `mouse.y` | カーソルの位置 |
| `mouse.inside` | カーソルがこの壁紙の上にあるか（ウィンドウに隠れていない部分だけ） |
| `mouse.left`, `mouse.right`, `mouse.middle` | ボタンを押しているか |

### サンドボックス

壁紙のスクリプトが使えるのは `math`、`string`、`table`、`bit` と基本関数だけ。
`io`、`os`、`require`、`load` 系、`debug`、`ffi`、`jit`、`getfenv` / `setfenv`、`string.dump` は使えない。
`print` は標準エラー出力に出る。

### エラーと無限ループ

- 読み込み時や実行時にエラーが起きたら、標準エラー出力に出して、直前のフレームを表示したまま止まる。ファイルを直して保存すれば再開する
- 1 回の呼び出しが長すぎる場合（読み込み 2 秒、毎フレーム 0.25 秒）はエラーとして止める
- ただし、JIT コンパイルされたループはこの仕組みでは止められない。スクリプトが 5 秒戻ってこなければ、kagegami は終了コード 3 で終了する（上の「自動起動」を参照）

## 開発

```sh
cargo test
# GPU 時間の計測（KAGEGAMI_BENCH_GLSL に Shadertoy 形式のシェーダーを渡すと比較できる）
cargo run --release --features gpu-bench -- --fps 1000 DP-1 examples/geometry.lua
```

## ライセンス

MIT

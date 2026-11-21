# kagegami

Lua で書けるライブ壁紙。Wayland（wlr-layer-shell）の背景レイヤーに描画する。

Lua の壁紙は、点や線などの図形を「1 フレームに一度だけ計算して、図形として GPU に送る」ので、
全ピクセルで同じ計算を繰り返すシェーダー方式の壁紙より GPU の負荷が小さい。
Shadertoy 形式の GLSL のシェーダーも、そのまま壁紙にできる（「シェーダーの壁紙」を参照）。

## ビルド

必要なもの: Rust、`wayland-client` / `wayland-egl` / EGL / GLES の開発ファイル、C コンパイラー（LuaJIT を同梱してビルドするため）。

実行には OpenGL ES 3.0 以上を使える GPU とドライバーが要る。GLES3 のコンテキストを作れない環境では、エラーを出して起動しない。

```sh
cargo build --release
cp target/release/kagegami ~/.local/bin/
```

## 使い方

```sh
kagegami [-o OUTPUT] [-f FPS] [-s SCALE] [-l background|bottom|top|overlay] <wallpaper.lua|wallpaper.glsl>
```

- 拡張子が `.glsl` か `.frag` のファイルはシェーダーの壁紙、それ以外は Lua の壁紙として読む

- `-o`, `--output`: 描画する出力の名前（`niri msg outputs` などで出る `DP-1` など）。省略すると全出力
- `-f`, `--fps`: フレームレートの上限（既定 60）
- `-s`, `--scale`: 出力の解像度に対する、描画する解像度の倍率（0.25〜2、既定 1）。後述の「解像度を下げて描く」を参照
- `-l`, `--layer`: 描画するレイヤー（既定 `background`）
- 壁紙のファイルを保存すると、自動で読み直す

例:

```sh
kagegami examples/geometry.lua          # 全出力に描画する
kagegami -o DP-1 examples/geometry.lua  # DP-1 だけに描画する
kagegami examples/waves.glsl            # シェーダーの壁紙
```

### 解像度を下げて描く

`-s 0.5` のように 1 より小さい倍率を指定すると、縦横それぞれその倍率の解像度で描画し、コンポジターが出力いっぱいに拡大して表示する。
GPU の負荷はおおよそピクセル数に比例するので、0.5 なら約 1/4 になる。1 より大きい倍率（最大 2）では大きく描いてから縮小するので、アンチエイリアスの代わりになる。

- 壁紙スクリプトから見える `screen.w`・`screen.h` とマウスの座標は、倍率に関係なく出力のピクセルサイズのまま。同じスクリプトが同じ見た目で描かれる
- 解像度を下げると、幅 1 ピクセルの線など細い図形が 1 ピクセルに満たなくなり、かすれたり途切れたりする
- 拡大・縮小のフィルターはコンポジターが決める
- 1 以外の倍率には、コンポジターが `wp_viewporter` に対応している必要がある。対応していなければエラーで終了する
- 倍率は起動 1 回につき 1 つで、描画するすべての出力に適用する。出力ごとに変えたいときは、`-o` で出力を指定して出力ごとに起動する

```sh
kagegami -o DP-1 -s 0.5 examples/geometry.lua &
kagegami -o HDMI-A-1 examples/geometry.lua &
```

### 自動起動

スクリプトが戻ってこなくなった場合（後述）、kagegami は終了コード 3 で終了する。
niri の `spawn-at-startup` は終了したプロセスを再起動しないので、終了コードが 3 のときだけ起動し直すループで包むとよい。
ほかの理由（Wayland への接続失敗など）で終了したときは、ループを抜ける。

`~/.config/niri/config.kdl`:

```kdl
spawn-at-startup "sh" "-c" "while :; do kagegami ~/.config/kagegami/geometry.lua; [ $? -eq 3 ] || break; sleep 2; done"
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
| `mouse.press_x`, `mouse.press_y` | 最後にドラッグを始めた（ボタンを押した）位置。離した後も残る。一度も押していなければ 0 |
| `mouse.dragging` | どれかのボタンを押している間 `true`。離すか、カーソルが壁紙の外に出ると `false` |

`press_x`・`press_y`・`dragging` は、シェーダーの `iMouse` と同じくボタンを区別しない。
左・右・中・4・5 のどれを押してもドラッグを始め直し、どれかを離すとドラッグが切れる。
座標は `mouse.x`・`mouse.y` と同じく左上が原点（`iMouse` と違い y を反転しない）。
例は `examples/slingshot.lua`。

### サンドボックス

壁紙のスクリプトが使えるのは `math`、`string`、`table`、`bit` と基本関数だけ。
`io`、`os`、`require`、`load` 系、`debug`、`ffi`、`jit`、`coroutine`、`getfenv` / `setfenv`、`string.dump` は使えない。
バイトコードは読み込めない（テキストのスクリプトだけ）。
`print` は標準エラー出力に出る。

描画関数（`line` など）は本体側で LuaJIT の `ffi` を使って頂点を直接書き込んでいるので、
1 図形あたり 10ns 弱で描ける（2 万個の図形で、Lua 側の処理は 1 フレーム約 0.2ms）。
`ffi` は描画関数の内側に閉じ込めてあり、壁紙のスクリプトからは触れない。

### エラーと無限ループ

- 読み込み時や実行時にエラーが起きたら、標準エラー出力に出して、直前のフレームを表示したまま止まる。ファイルを直して保存すれば再開する
- 1 回の呼び出しが長すぎる場合（読み込み 2 秒、毎フレーム 0.25 秒）はエラーとして止める
- 1 フレームに描ける図形は 100 万個まで。超えたらエラーとして止める
- ただし、JIT コンパイルされたループのうち、描画関数を呼ばないものはこの仕組みでは止められない。スクリプトが 5 秒戻ってこなければ、kagegami は終了コード 3 で終了する（上の「自動起動」を参照）

## シェーダーの壁紙

Shadertoy の「Image」タブと同じ形で書く。`mainImage` を定義したファイルを、そのまま渡せばよい。

```glsl
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    fragColor = vec4(uv, 0.5 + 0.5 * sin(iTime), 1.0);
}
```

GLSL ES 3.00（`#version 300 es`）として、出力ごとにコンパイルする。出力するアルファは無視して 1 にする。

| uniform | 内容 |
|---|---|
| `vec3 iResolution` | 画面のピクセルサイズ（z は 1） |
| `float iTime` | 読み込んでからの秒数 |
| `float iTimeDelta` | 前のフレームからの秒数 |
| `int iFrame` | 読み込んでからのフレーム数 |
| `vec4 iMouse` | マウス（下を参照） |
| `vec4 iDate` | (年, 月 - 1, 日, 0 時からの秒数)。ローカル時刻 |

`iChannel0`〜`3`（テクスチャー・音声など）、マルチパス（Buffer A〜D）、Common タブは使えない。

### iMouse

Shadertoy と同じく、ボタンを押している間のドラッグを表す。座標は左下が原点のピクセル。

- `xy`: ドラッグ中の今の位置。ボタンを離した後は、最後の位置のまま
- `zw`: ドラッグを始めた位置。`z` はボタンを押している間だけ正、`w` は押した最初のフレームだけ正。それ以外は負になる（絶対値は始めた位置のまま）
- どのボタン（左・右・中・サイドボタン）でも反応する。押すたびにその位置からドラッグを始め直し、どれかのボタンを離すとドラッグが切れる
- コンポジターからカーソルが壁紙を離れたと知らされたら（Wayland の `Leave`）、ボタンを離した扱いにする。ドラッグ中に壁紙の外の座標が届いたときは、その座標をそのまま渡す
- 一度も押していなければ、すべて 0

### エラーと重すぎるシェーダー

- コンパイルエラーは標準エラー出力に出して、直前のフレームを表示したまま止まる。ファイルを直して保存すれば再開する
- 1 フレームの GPU 時間が 0.25 秒を超えたら、エラーとして同じように止める。GPU 時間は `GL_EXT_disjoint_timer_query` で測るので、止まるのは超えたフレームの数フレーム後になる。この拡張機能がない環境では、起動時に警告を出し、時間を見張らずに描く
- 終わらないループ（`while (true)` など）は止められない。描画が戻らなくなり、最後は GPU ドライバーが GPU をリセットする。そのあいだ、画面全体が固まることがある

## 開発

```sh
cargo test
# GPU 時間の計測（KAGEGAMI_BENCH_GLSL に Shadertoy 形式のシェーダーを渡すと比較できる）
cargo run --release --features gpu-bench -- --fps 1000 -o DP-1 examples/geometry.lua
```

## ライセンス

MIT

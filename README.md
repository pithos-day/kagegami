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
kagegami [-o OUTPUT] [-f FPS] [-s SCALE] [-l background|bottom|top|overlay] [-p] <wallpaper.lua|wallpaper.glsl>
```

- 拡張子が `.glsl` か `.frag` のファイルはシェーダーの壁紙、それ以外は Lua の壁紙として読む

- `-o`, `--output`: 描画する出力の名前（`niri msg outputs` などで出る `DP-1` など）。省略すると全出力
- `-f`, `--fps`: フレームレートの上限（既定 60）
- `-s`, `--scale`: 出力の解像度に対する、描画する解像度の倍率（0.25〜2、既定 1）。後述の「解像度を下げて描く」を参照
- `-l`, `--layer`: 描画するレイヤー（既定 `background`）
- `-p`, `--perf`: 壁紙の負荷を標準エラー出力に 1 秒ごとに出す。後述の「負荷を見る」を参照
- 壁紙のファイルを保存すると、自動で読み直す。シェーダーの壁紙が使う画像を保存したときも読み直す

例:

```sh
kagegami examples/geometry.lua          # 全出力に描画する
kagegami -o DP-1 examples/geometry.lua  # DP-1 だけに描画する
kagegami examples/waves.glsl            # シェーダーの壁紙
```

### 解像度を下げて描く

`-s 0.5` のように 1 より小さい倍率を指定すると、縦横それぞれその倍率の解像度で描画し、コンポジターが出力いっぱいに拡大して表示する。
GPU の負荷はおおよそピクセル数に比例するので、0.5 なら約 1/4 になる。1 より大きい倍率（最大 2）では大きく描いてから縮小するので、アンチエイリアスの代わりになる。

- Lua の壁紙スクリプトから見える `screen.w`・`screen.h` とマウスの座標は、倍率に関係なく出力のピクセルサイズのまま。同じスクリプトが同じ見た目で描かれる
- シェーダーの壁紙では、`iResolution`・`fragCoord`・`iMouse` は描画するバッファーのピクセル（出力のピクセルサイズ × 倍率）になる。`fragCoord / iResolution` のように解像度で割って書けば同じ見た目で描かれるが、半径などをピクセルの固定値で書くと、倍率によって画面上の大きさが変わる
- 解像度を下げると、幅 1 ピクセルの線など細い図形が 1 ピクセルに満たなくなり、かすれたり途切れたりする
- 拡大・縮小のフィルターはコンポジターが決める
- 1 以外の倍率には、コンポジターが `wp_viewporter` に対応している必要がある。対応していなければエラーで終了する
- 倍率は起動 1 回につき 1 つで、描画するすべての出力に適用する。出力ごとに変えたいときは、`-o` で出力を指定して出力ごとに起動する

```sh
kagegami -o DP-1 -s 0.5 examples/geometry.lua &
kagegami -o HDMI-A-1 examples/geometry.lua &
```

### 負荷を見る

`-p`（`--perf`）を付けると、壁紙の負荷を 1 秒ごとに標準エラー出力へ出す。壁紙を作りながら、どこまで重くすると止められるか（「エラーと無限ループ」「エラーと重すぎるシェーダー」の制限）を確かめるためのもの。

```sh
kagegami -p examples/geometry.lua
```

標準エラー出力が端末なら、出力ごとに 1 行と、プロセス全体の 1 行を、一番下で書き換え続ける。エラーや `print` のメッセージは、その上に流れる。

```
DP-1        59.9/60 fps  lua   3.20 ms (max   8.10 ms,   3%)  gpu   1.40 ms (max   2.00 ms)  quads    12,345 (  1%)
HDMI-A-1    60.0/60 fps  lua   2.90 ms (max   4.00 ms,   2%)  gpu   1.10 ms (max   1.30 ms)  quads    12,345 (  1%)
cpu  12%  gpu gfx   8%  rss   48.2 MiB  lua    2.1 MiB
```

- `fps`: 直前の 1 秒に描いたフレームの数と、`--fps` の値。出力が隠れていて描いていないときは 0 になる
- `lua`: Lua の壁紙で `update` と `draw` にかかった時間。直前の 1 秒の平均と、読み込んでからの最大。% は最大を 1 フレームの制限（0.25 秒）で割ったもの
- `gpu`: 1 フレームの描画にかかった GPU 時間。直前の 1 秒に結果が届いたフレームの平均と、読み込んでからの最大。シェーダーの壁紙では、最大を制限（0.25 秒）で割った % も出す。`GL_EXT_disjoint_timer_query` がない環境では出さない
- `quads`: 直前の 1 秒で最も多かった、1 フレームの図形の数。% は上限（100 万）で割ったもの
- `stopped (error)`: 壁紙がエラーで止まっている
- 最後の行: プロセス全体の CPU 使用率（1 コアを 100%）、GPU 使用率、RSS、Lua の壁紙が使っているメモリ（全出力の合計）
- `gpu gfx 8%` のような最後の行の GPU 使用率: 直前の 1 秒に、kagegami が GPU のエンジンを使っていた時間の割合。エンジンごとに名前を付けて出し、使っていないエンジンは出さない。エンジンの名前はドライバーによって違う（amdgpu は `gfx` など）
  - カーネルの DRM の fdinfo（`/proc/self/fdinfo`）から読む。使用時間を ns で出すドライバーでしか出ない。動くことを確かめたのは amdgpu だけ。NVIDIA のプロプライエタリドライバーや、サイクル数で出すドライバー（xe など）では出さない
  - 出力ごとの `gpu`（1 フレームの描画を timer query で測った時間）とは別に、カーネルが数えたプロセス全体の値。起動して最初の 1 回は、比べる前の値がないので出さない
- 壁紙のファイルを保存して読み直すと、最大値も消して測り直す

標準エラー出力が端末でなければ（ファイルにリダイレクトしたときなど）、同じ値を key=value の形で 1 秒ごとに 1 行ずつ出す。

```
out=DP-1 fps=59.9/60 lua_avg=3.20ms lua_max=8.10ms lua_limit=3% gpu_avg=1.40ms gpu_max=2.00ms quads=12345 quads_limit=1% out=HDMI-A-1 ... cpu=12% gpu_gfx=8% rss=48.2MiB lua_mem=2.1MiB
```

`--perf` は、描画を待たせる `glFinish` を使わずに測るので、付けても描画の進み方は変わらない。ただし、Lua の壁紙では GPU 時間を測るぶんの処理が増える。開発用の `--features gpu-bench`（「開発」を参照）とは違い、決まったフレーム数を測って終了することはない。

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
壁紙のファイルを保存して読み直すと、乱数の種も新しくなる。読み直す前と同じ乱数の並びにはならない。

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
| `rect(x, y, w, h, angle)` | 塗りつぶした矩形。`angle`（ラジアン）を渡すと、矩形の中心のまわりに回す（画面上で時計回り）。`angle` は省略できる（下を参照） |

`rect` は `angle` を渡したときだけ、縁にアンチエイリアスを付けて描く（値が 0 でも）。
`angle` を省くと、今までどおり縁にアンチエイリアスを付けずに描く。
そのため、座標が整数でないときは `rect(x, y, w, h)` と `rect(x, y, w, h, 0)` の縁の見た目が少し違う（後者がわずかにぼやける）。
回転させる矩形は、角度が 0 を通るときも見た目が変わらないように、常に `angle` を渡すとよい。例は `examples/tiles.lua`。

描画関数に、表に書いた数より多い引数を渡すとエラーになる（`circle: too many arguments (max 3)` など）。
後ろに付いた `nil` は数えないので、`rect(x, y, w, h, nil)` は `rect(x, y, w, h)` と同じ。途中の `nil`（`rect(x, y, w, h, nil, 1)` など）はエラーになる。

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

- 起動時に壁紙のファイルを読めなければ（ファイルが無いなど）、エラーを出して終了コード 1 で終了する。実行中にファイルが消えた場合は、保存の途中とみなして、また現れるまで待つ
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
| `vec3 iResolution` | 描画するバッファーのピクセルサイズ（z は 1）。`--scale` が 1 なら画面のピクセルサイズ |
| `float iTime` | 読み込んでからの秒数 |
| `float iTimeDelta` | 前のフレームからの秒数 |
| `int iFrame` | 読み込んでからのフレーム数 |
| `vec4 iMouse` | マウス（下を参照） |
| `vec4 iDate` | (年, 月 - 1, 日, 0 時からの秒数)。ローカル時刻 |
| `sampler2D iChannel0`〜`3` | 画像（下の「画像」を参照）。指定していないチャンネルは黒 |
| `vec3 iChannelResolution[4]` | 画像のピクセルサイズ（z は 1）。指定していないチャンネルは (0, 0, 0) |

`iChannel0`〜`3` に渡せるのは画像（PNG・JPEG）だけ。音声・キューブマップ・動画、マルチパス（Buffer A〜D）、Common タブは使えない。

### 画像

ファイルの先頭のコメントに `@iChannel0`〜`@iChannel3` で画像を指定する。例は `examples/flow.glsl`。

```glsl
// @iChannel0 photo.jpg
// @iChannel1 "my noise.png" filter=nearest wrap=clamp vflip=false
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = texture(iChannel0, fragCoord / iResolution.xy);
}
```

- 書ける場所は、ファイルの先頭から、コメント（`//` と `/* */`）と空行が続く間。ライセンス表記の `/* */` の後ろにも書ける。それより後ろの行に書いた指定はエラーになる
- パスは、壁紙のファイルがあるディレクトリからの相対パス、絶対パス、`~/` で始まるパス（ホームディレクトリから）のどれか。空白を含むパスは `"` で囲む
- 形式は PNG と JPEG。JPEG の EXIF の向き（Orientation）は反映する。縦横はそれぞれ GPU のテクスチャーの上限（`GL_MAX_TEXTURE_SIZE`）まで
- オプション（どれも省略できる）:

| オプション | 値 | 既定 |
|---|---|---|
| `filter` | `nearest` / `linear` / `mipmap` | `mipmap` |
| `wrap` | `clamp` / `repeat` | `repeat` |
| `vflip` | `true` / `false` | `true` |

- `vflip=true` は、画像の上が画面の上に来る向き（`fragCoord / iResolution.xy` で画像の左下が画面の左下に来る）
- 既定の値は Shadertoy と同じつもりだが、Shadertoy のサイトでは確かめていない
- 大文字・小文字を問わず `@ichannel` か `@image` で始まる名前は指定として確かめ、正しくなければエラーにする（`@iChannel4`・`@ichannel0` など）。それ以外の `@author` などは、ただのコメントとして無視する
- 画像はすべての出力で 1 回だけ読み込み、出力の間で共有する。読み込みは、壁紙のファイルを読むときに行う（読み込む間は描画が止まる。4K の JPEG のデコードは、#36 の計測で約 20ms）

### iMouse

Shadertoy と同じく、ボタンを押している間のドラッグを表す。座標は `fragCoord` と同じく、左下が原点の、描画するバッファーのピクセル。

- `xy`: ドラッグ中の今の位置。ボタンを離した後は、最後の位置のまま
- `zw`: ドラッグを始めた位置。`z` はボタンを押している間だけ正、`w` は押した最初のフレームだけ正。それ以外は負になる（絶対値は始めた位置のまま）
- どのボタン（左・右・中・サイドボタン）でも反応する。押すたびにその位置からドラッグを始め直し、どれかのボタンを離すとドラッグが切れる
- コンポジターからカーソルが壁紙を離れたと知らされたら（Wayland の `Leave`）、ボタンを離した扱いにする。ドラッグ中に壁紙の外の座標が届いたときは、その座標をそのまま渡す
- 一度も押していなければ、すべて 0

### エラーと重すぎるシェーダー

- 起動時にファイルを読めなければ、Lua の壁紙と同じように終了コード 1 で終了する
- 画像の指定の書き間違いや、画像を読めない（ファイルがない・壊れている・大きすぎる）ときは、コンパイルエラーと同じように止まる。画像を直して保存すれば再開する。指定した画像は、読めなかったものも見張っている
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

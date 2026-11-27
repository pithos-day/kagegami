# GLSL の壁紙

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
| `sampler2D iChannel0`〜`3` | 画像（下の「画像」を参照）、バッファー（下の「バッファーと Common」を参照）、キーボード（下の「キーボード」を参照）。指定していないチャンネルは黒。キューブマップを指定したチャンネルは `samplerCube`（下の「キューブマップ」を参照）、3D ノイズを指定したチャンネルは `sampler3D`（下の「3D ノイズ」を参照） |
| `vec3 iChannelResolution[4]` | 画像・バッファー・キーボードのピクセルサイズ（z は 1）。キューブマップは (面のサイズ, 面のサイズ, 1)。3D ノイズは (32, 32, 32)。指定していないチャンネルは (0, 0, 0) |

`iChannel0`〜`3` に渡せるのは、画像（PNG・JPEG）、キューブマップ（正距円筒図法の画像から作る）、バッファー（Buffer A〜D）、組み込みの 3D ノイズ、キーボード。音声・動画と、キューブマップに描くパス（Shadertoy の Cubemap A）は使えない。

## 画像

ファイルの先頭のコメントに `@iChannel0`〜`@iChannel3` で画像を指定する。例は `examples/flow.glsl`。

```glsl
// @iChannel0 photo.jpg
// @iChannel1 "my noise.png" filter=nearest wrap=clamp vflip=false
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = texture(iChannel0, fragCoord / iResolution.xy);
}
```

- 書ける場所は、ファイルの先頭から、コメント（`//` と `/* */`）と空行が続く間。ライセンス表記の `/* */` の後ろにも書ける。それより後ろの行に書いた指定はエラーになる
- パスは、壁紙のファイルがあるディレクトリからの相対パス、絶対パス、`~/` で始まるパス（ホームディレクトリから）のどれか。空白を含むパスは `"` で囲む。zip ファイルで渡したときは、`~/` と絶対パスは書けない（使い方の詳細の[「ディレクトリと zip ファイル」](usage.md#ディレクトリと-zip-ファイル)を参照）
- 形式は PNG と JPEG。JPEG の EXIF の向き（Orientation）は反映する。縦横はそれぞれ GPU のテクスチャーの上限（`GL_MAX_TEXTURE_SIZE`）まで
- オプション（どれも省略できる）:

| オプション | 値 | 既定 |
|---|---|---|
| `filter` | `nearest` / `linear` / `mipmap` | `mipmap` |
| `wrap` | `clamp` / `repeat` | `repeat` |
| `vflip` | `true` / `false` | `true` |

- `vflip=true` は、画像の上が画面の上に来る向き（`fragCoord / iResolution.xy` で画像の左下が画面の左下に来る）
- 既定の値は、Shadertoy のサイトで iChannel に新しく画像を指定したときの設定（mipmap・repeat・vflip 有効）と同じ
- 大文字・小文字を問わず `@ichannel`・`@image`・`@buffer`・`@common` で始まる名前は指定として確かめ、正しくなければエラーにする（`@iChannel4`・`@ichannel0`・`@bufferE` など）。それ以外の `@author` などは、ただのコメントとして無視する
- 画像はすべての出力で 1 回だけ読み込み、出力の間で共有する。読み込みは、壁紙のファイルを読むときに行う（読み込む間は描画が止まる。4K の JPEG のデコードは、#36 の計測で約 20ms）

## バッファーと Common

Shadertoy の Buffer A〜D と Common のタブを、それぞれ別のファイルに書く。Image のファイル（kagegami に渡すファイル。ディレクトリか zip ファイルを渡すときは、その中の `main.glsl` か `main.frag`）の先頭のコメントで、バッファーと Common のファイルを宣言し、`@iChannel` にバッファーの名前を書いて読む。例は `examples/ink/`（`main.glsl` が Image のファイル）。

```glsl
// main.glsl（Image）
// @bufferA sim.glsl
// @common common.glsl
// @iChannel0 bufferA
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = texture(iChannel0, fragCoord / iResolution.xy);
}
```

```glsl
// sim.glsl（Buffer A）。自分を読むと前のフレームの結果になる
// @iChannel0 bufferA filter=nearest
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec4 prev = texelFetch(iChannel0, ivec2(fragCoord), 0);
    fragColor = iFrame == 0 ? vec4(0.0) : prev + vec4(iTimeDelta);
}
```

- `@bufferA`〜`@bufferD` と `@common` は、Image のファイルにだけ書ける。パスの書き方は画像と同じ（Image のファイルがあるディレクトリから）
- `@iChannel0 bufferA` のように、チャンネルにバッファーの名前（`bufferA`〜`bufferD`）を書く。Image のファイルでも、バッファーのファイルでも書ける。宣言していないバッファーはエラーになる。`bufferA` という名前の画像のファイルは `./bufferA` と書く
- バッファーのファイルは Shadertoy の Buffer のタブと同じ形（`mainImage` を定義する）で、自分の `@iChannel` を書ける。画像のパスは、そのファイルがあるディレクトリから探す
- Common のファイルの中身は、すべてのパスの uniform の宣言の後、パスのソースの前に入る（Shadertoy と同じ）。Common のファイルには指定を書けない
- 毎フレーム、A → B → C → D → Image の順に描く。自分より前のバッファーを読むと今のフレームの結果、自分か後ろのバッファーを読むと前のフレームの結果になる（Shadertoy と同じ）
- バッファーの形式は RGBA32F（Shadertoy と同じ）。書いた値はアルファも含めてそのまま残り、0〜1 に丸められない
- バッファーのサイズは描画するバッファーのピクセル（`iResolution` と同じ）で、出力ごとに持つ。最初と、壁紙を読み直したとき（`iFrame` が 0 に戻る）は (0, 0, 0, 0) から始まる。出力のサイズが変わったときは、重なる範囲（左下から）の中身を写し、ほかは 0 にする
- オプション（どれも省略できる）:

| オプション | 値 | 既定 |
|---|---|---|
| `filter` | `nearest` / `linear` / `mipmap` | `linear` |
| `wrap` | `clamp` / `repeat` | `clamp` |

- 既定の値は、Shadertoy のサイトで iChannel にバッファーを指定したときの設定と同じ。`vflip` は書けない（エラーになる）
- `filter=mipmap` で読むバッファーは、描くたびにミップマップを作る。その分 GPU 時間が増える。どのパスも mipmap で読まないバッファーでは作らない
- GPU が `GL_OES_texture_float_linear` に対応していなければ、起動時に警告を出し、`linear`・`mipmap` を `nearest` として読む（Shadertoy と同じ）
- Common のエラーは、行番号の前の数字が 1 になる（Mesa では `1:行(列)`。パスのファイルは `0:行(列)`）。Common の中で閉じていない括弧などは、続くパスのファイルの位置でエラーになることがある
- VRAM: 1 つのバッファーにつき、RGBA32F のテクスチャーを 2 枚（前のフレームの結果と、今描いている方）、出力ごとに持つ。1 枚は 横 × 縦 × 16 バイト。#58 の計測では、バッファーを使わない壁紙と比べて、1920×1080 でバッファー 1 つが約 66MiB、4 つが約 255MiB、3840×2160 で 4 つが約 1.0GiB 増えた（1 つの出力あたり）。`filter=mipmap` で読むバッファーは、計算ではさらに約 1/3 増える（測っていない）。`--scale 0.5` で約 1/4 になる

## キューブマップ

画像の指定に `type=cubemap` を付けると、正距円筒図法（縦横比 2:1 のパノラマ）の画像からキューブマップを作り、`samplerCube` で読める。360° 写真などのパノラマを、PNG か JPEG でそのまま使える（#68）。例は `examples/aero.glsl`。

```glsl
// @iChannel0 sky.jpg type=cubemap
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 p = (2.0 * fragCoord - iResolution.xy) / iResolution.y;
    vec3 rd = normalize(vec3(p, -1.5));  // -Z を向くカメラ
    fragColor = texture(iChannel0, rd);
}
```

- 画像の向き: 画像の中心が -Z、右へ 1/4 進んだ位置が +X、上が +Y。経度 lon（-π〜π、中心が 0）・緯度 lat（-π/2〜π/2、上が正）の画素は、方向 `(sin(lon)·cos(lat), sin(lat), -cos(lon)·cos(lat))` に当たる。Shadertoy の組み込みのキューブマップを、この式で正距円筒図法に広げた絵と、チャンネルのサムネイルが一致する（#68）
- 画像の縦横比はちょうど 2:1 にする。そうでなければエラーになる
- 面のサイズは画像の幅 ÷ 4（4 で割り切れなければ切り捨て）。縦横が `GL_MAX_TEXTURE_SIZE` を超える画像と、面のサイズが `GL_MAX_CUBE_MAP_TEXTURE_SIZE` を超える画像はエラーになる
- オプションは `filter`（`nearest` / `linear` / `mipmap`、既定 `mipmap`。Shadertoy と同じ）だけ。`wrap` は面の境目が常につながるので意味がなく、`vflip` は Shadertoy のキューブマップにない。どちらも書くとエラーになる
- `textureSize(iChannelN, 0)` は面のサイズを返す（Shadertoy では常に 1 になる）
- 読み込むときに 1 回だけ、GPU で画像を 6 面に写す。`filter=mipmap` なら、その後にミップマップを作る
- VRAM: 計算では、幅 W の画像で、キューブマップ（mipmap 付き）は約 2W² バイト（同じ画像を普通の画像として mipmap 付きで読むと約 2.7W² バイト）。写す間だけ、元の画像のテクスチャー（2W² バイト）も持つ（どちらも測っていない）
- Lua の壁紙では使えない

## 3D ノイズ

`@iChannel0`〜`@iChannel3` に `noise3d-grey` か `noise3d-rgba` と書くと、kagegami が作る 32×32×32 のノイズを `sampler3D` で読める。Shadertoy の組み込みの 3D ノイズ（グレーと RGBA の 2 種類）の代わりに使う。例は `examples/fog.glsl`。

```glsl
// @iChannel0 noise3d-grey
// @iChannel1 noise3d-rgba filter=nearest
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec3 p = vec3(fragCoord / iResolution.y, iTime * 0.1);
    fragColor = vec4(vec3(texture(iChannel0, p).r), 1.0);
}
```

| 名前 | 形式 | 中身 |
|---|---|---|
| `noise3d-grey` | R8（`.r` だけが値で、`.gba` は (0, 0, 1)） | `noise3d-rgba` のバイト列の先頭 32³ バイト |
| `noise3d-rgba` | RGBA8 | ボクセルごと・チャンネルごとに 0〜1 の一様な乱数 |

- 中身は Shadertoy のファイルとは違う。どちらも、隣り合うボクセルやチャンネルの間に相関のない一様な乱数（#70）
- 乱数の種は固定なので、起動ごと・出力ごと・読み直すごとに同じノイズになる
- オプションは `filter`（`nearest` / `linear` / `mipmap`、既定 `mipmap`）と `wrap`（`clamp` / `repeat`、既定 `repeat`）。`wrap` は 3 軸（s・t・r）すべてにかかる。`vflip` は書けない（エラーになる）
- 名前は大文字・小文字も含めて正確に書く。`noise3d` で始まるほかの名前（`noise3d-gray`・`Noise3D-rgba` など）はエラーになる。`noise3d` で始まる名前の画像のファイルは `./noise3d-rgba.png` のように書く
- 3D ノイズを指定したチャンネルは `sampler3D` になるので、`texture(iChannelN, vec3)` で読む。`vec2` で読むとコンパイルエラーになる
- ノイズは最初に使うときに 1 回だけ作る（#70 の計測で、作るのに約 35µs、GPU に送るのに 1 枚あたり約 2ms 以下）

## キーボード

`@iChannel0 keyboard` のように、チャンネルに `keyboard` と書くと、Shadertoy のキーボードと同じテクスチャーを読める。例は `examples/keyboard.glsl`。

```glsl
// @iChannel0 keyboard
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    float space = texelFetch(iChannel0, ivec2(32, 0), 0).x;
    fragColor = vec4(vec3(space), 1.0);
}
```

- テクスチャーは 256×3 で、値は赤のチャンネルの 0 か 1。x はブラウザーの `keyCode`（`A`〜`Z` は 65〜90、`0`〜`9` は 48〜57、Space は 32、矢印キーは 37〜40）
- y = 0 は押している間、y = 1 は押したフレームだけ、y = 2 は押すたびに切り替わる
- キーはキーボードの配列に従う（キーに書かれた文字の番号になる）。Shift などを押していても同じ番号になる。左右の Shift・Ctrl・Alt は同じ番号（16・17・18）
- US 配列にない記号のキー（JIS 配列の `@` など）や、日本語の入力のキー（変換など）は、番号がないので出ない
- `filter` などの設定は書けない（nearest・clamp で読む）。`keyboard` という名前の画像のファイルは `./keyboard` と書く
- バッファーのファイルにも書ける。Common のファイルには書けない

キーが届くのは、壁紙にキーボードのフォーカスがあるときだけ。`keyboard` を指定した壁紙を読み込むと、壁紙の layer surface を `keyboard_interactivity` の `on_demand` にして、フォーカスを受け取れるようにする。フォーカスがいつ来るか（壁紙をクリックしたときなど）はコンポジター次第。ウィンドウがフォーカスを持っている間は、キーは壁紙に届かない。

- キーが届くのは、フォーカスを持っている出力の壁紙だけ（マウスと同じ）。ほかの出力の壁紙では、キーを押していない扱いになる
- フォーカスが外れたら、押しているキーをすべて離した扱いにする
- キーボードを使わない壁紙では、今までどおり `none` のままにし、フォーカスを受け取らない
- コンポジターの wlr-layer-shell がバージョン 4 より古く `on_demand` を使えなければ、警告を出してキーを使えないままにする

## iMouse

Shadertoy と同じく、ボタンを押している間のドラッグを表す。座標は `fragCoord` と同じく、左下が原点の、描画するバッファーのピクセル。

- `xy`: ドラッグ中の今の位置。ボタンを離した後は、最後の位置のまま
- `zw`: ドラッグを始めた位置。`z` はボタンを押している間だけ正、`w` は押した最初のフレームだけ正。それ以外は負になる（絶対値は始めた位置のまま）
- どのボタン（左・右・中・サイドボタン）でも反応する。押すたびにその位置からドラッグを始め直し、どれかのボタンを離すとドラッグが切れる
- コンポジターからカーソルが壁紙を離れたと知らされたら（Wayland の `Leave`）、ボタンを離した扱いにする。ドラッグ中に壁紙の外の座標が届いたときは、その座標をそのまま渡す
- 一度も押していなければ、すべて 0

## エラーと重すぎるシェーダー

- 起動時にファイルを読めなければ、Lua の壁紙と同じように終了コード 1 で終了する
- 画像の指定の書き間違いや、画像を読めない（ファイルがない・壊れている・大きすぎる）ときは、コンパイルエラーと同じように止まる。画像を直して保存すれば再開する。指定した画像は、読めなかったものも見張っている（zip ファイルで渡したときは、zip ファイルを見張る）
- バッファー・Common のファイルを読めないとき、宣言していないバッファーを読もうとしたときも、同じように止まる。宣言したファイルは、読めなかったものも見張っている（zip ファイルで渡したときは、zip ファイルを見張る）
- バッファーを使う壁紙は、GPU が `GL_EXT_color_buffer_float`（RGBA32F のテクスチャーへの描画）に対応していなければ、エラーとして止まる。VRAM が足りずにバッファーを作れないときも、同じように止まる
- コンパイルエラーは標準エラー出力に出して、直前のフレームを表示したまま止まる。ファイルを直して保存すれば再開する
- 1 フレームの GPU 時間（バッファーを使う壁紙では、すべてのパスとミップマップの作成を合わせた時間）が 0.25 秒を超えたら、エラーとして同じように止める。GPU 時間は `GL_EXT_disjoint_timer_query` で測るので、止まるのは超えたフレームの数フレーム後になる。この拡張機能がない環境では、起動時に警告を出し、時間を見張らずに描く
- 終わらないループ（`while (true)` など）は止められない。描画が戻らなくなり、最後は GPU ドライバーが GPU をリセットする。そのあいだ、画面全体が固まることがある

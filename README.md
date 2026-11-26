# kagegami

Lua で書けるライブ壁紙。Wayland（wlr-layer-shell）の背景レイヤーに描画する。

Lua の壁紙は、点や線などの図形を「1 フレームに一度だけ計算して、図形として GPU に送る」ので、
全ピクセルで同じ計算を繰り返すシェーダー方式の壁紙より GPU の負荷が小さい。
Shadertoy 形式の GLSL のシェーダーも、そのまま壁紙にできる。
詳しくは[ドキュメント](docs/README.md)を参照。

## ビルド

必要なもの: Rust、`wayland-client` / `wayland-egl` / EGL / GLES の開発ファイル、C コンパイラー（LuaJIT を同梱してビルドするため）。

実行には OpenGL ES 3.0 以上を使える GPU とドライバーが要る。GLES3 のコンテキストを作れない環境では、エラーを出して起動しない。

```sh
cargo build --release
cp target/release/kagegami ~/.local/bin/
```

## 使い方

```sh
kagegami [-o OUTPUT] [-f FPS] [-s SCALE] [-l background|bottom|top|overlay] [-p] <wallpaper.lua|wallpaper.glsl|directory|wallpaper.zip>
```

- 拡張子が `.glsl` か `.frag` のファイルはシェーダーの壁紙、それ以外は Lua の壁紙として読む
- ディレクトリか zip ファイルを渡すと、その中の `main.glsl`・`main.frag`・`main.lua` のどれか 1 つを壁紙のファイルとして読む

- `-o`, `--output`: 描画する出力の名前（`niri msg outputs` などで出る `DP-1` など）。省略すると全出力
- `-f`, `--fps`: フレームレートの上限（既定 60）
- `-s`, `--scale`: 出力の解像度に対する、描画する解像度の倍率（0.25〜2、既定 1）
- `-l`, `--layer`: 描画するレイヤー（既定 `background`）
- `-p`, `--perf`: 壁紙の負荷を標準エラー出力に 1 秒ごとに出す
- 壁紙のファイルを保存すると、自動で読み直す。壁紙が使う画像（Lua の `@image`、シェーダーの `@iChannel`）や、シェーダーのバッファー・Common のファイル（`@bufferA`〜`@bufferD`・`@common`）を保存したときも読み直す

例:

```sh
kagegami examples/geometry.lua          # 全出力に描画する
kagegami -o DP-1 examples/geometry.lua  # DP-1 だけに描画する
kagegami examples/waves.glsl            # シェーダーの壁紙
kagegami examples/ink                   # バッファーを使うシェーダーの壁紙（ディレクトリ）
kagegami ink.zip                        # 同じものを zip にしたもの
```

## 開発

```sh
cargo test
# GPU 時間の計測（KAGEGAMI_BENCH_GLSL に Shadertoy 形式のシェーダーを渡すと比較できる）
cargo run --release --features gpu-bench -- --fps 1000 -o DP-1 examples/geometry.lua
```

## ライセンス

MIT

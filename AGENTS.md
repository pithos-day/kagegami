# AGENTS.md

このリポジトリは、本人（リポジトリの持ち主）が判断し、AI が実装を手伝う形で開発している。判断と検証の過程を Issue と PR に残す。

## 本人が決めること（AI が決めない）

- 仕様・スコープ・やらないこと、受入条件
- 技術・構成の採否、トレードオフの判断
- マージするかどうかと順番、リリース、切り戻し

上のどれかが必要になったら、選択肢を比較表（効果・不利益・根拠）で示して止まり、本人の判断を待つ。判断は Issue の「判断記録」テンプレートで残す。

## 記録の書き方

- 記録（Issue・PR・コメント・docs・コード内のコメント）には、判断・仕様・事実・検証の結果だけを書く。誰が書いた・提案したか（「AI の案」など）と、依頼の原文は書かない
- していない判断・確認・再実行を、したと書かない。まだ決めていない点は「未決」と書く
- 記録の種類はラベルで表す（`判断`・`調査`・`計測`）。タイトルに `[判断]` などの接頭辞を付けない。判断が確定したら `確定` ラベルを付ける
- 検証の結果は、対象の commit と対応付けて書く。判断が変わったら、上書きせず日時と理由をコメントで追記する
- AI が書いた Issue・PR の本文とコメントは、末尾に `Model: <モデルの表示名>` を 1 行付ける（コミットの `Commit-Model:` と同じ表記。例: `Model: Claude Opus 5.5`）。本文の中には書かない。人が書いたものには付けない（#3）

## 作業の規則

- すべての変更を Issue → PR → 本人の指示でマージ、の順で入れる。`main` に直接 push しない
- 実装を始める前に、対応する Issue があることを確かめ、PR からリンクする（`Closes #番号` など）
- 調査・計測の途中経過は Issue のコメントに追記する。本文を上書きして過程を消さない
- PR は `.github/pull_request_template.md` を埋める。マージは本人が指示したものだけを、マージコミットで行う
- バグを直すときは、先に「修正前に失敗するテスト」を書き、修正後に成功することを示す
- 計測は、改善前の版を残してから始める。数値を推測で書かない
- 誤りが見つかったら、`誤りの修正` ラベルで「誤っていた内容 → 根拠 → 修正 → 修正前後の検証」を残す
- コミットは Conventional Commits。Issue・PR・ドキュメントは日本語で書く
- 作業は Issue ごとに git worktree を作って行う。worktree は ghq root の外の `~/worktrees/github.com/pithos-day/kagegami/<topic>` に作り、新しいブランチは `git fetch origin` の後に `origin/main` から切る（`git worktree add ~/worktrees/github.com/pithos-day/kagegami/<topic> -b <branch> origin/main`）。メインのチェックアウトでブランチを切り替えない。マージした後は `git worktree remove` で消す（#66、#76）

## kagegami

Rust（edition 2024）+ LuaJIT（mlua）の Wayland ライブ壁紙。wlr-layer-shell の背景レイヤーに EGL / GLES3 で描画する。

- ロジックを変えたら `cargo test`、push の前に `cargo fmt --check` と `cargo test` を実行する
- 壁紙スクリプトのサンドボックスを保つ。使えるのは `math`・`string`・`table`・`bit` と基本関数だけで、`io`・`os`・`require`・`load` 系・`debug`・`ffi`・`jit`・`coroutine`・`getfenv` / `setfenv`・`string.dump` とバイトコードの読み込みは使えない。`ffi` は描画関数の内側に閉じ込め、スクリプトから触れさせない
- 暴走したスクリプトへの制限を保つ（読み込み 2 秒・毎フレーム 0.25 秒・1 フレーム 100 万図形、5 秒戻らなければ終了コード 3 で終了）
- シェーダーの壁紙は、1 フレームの GPU 時間が 0.25 秒を超えたら止める（`GL_EXT_disjoint_timer_query` で測る）
- 仕様（コマンドのオプション・Lua の API・シェーダーの uniform と指定・制限・サンドボックス・エラーの扱い）を変えたら、`docs/` の該当ファイル（`usage.md`・`lua.md`・`shader.md`）も同じ PR で合わせて変える。ファイルや節を足したら、一覧の `docs/README.md` も変える
- 性能の変更は `--features gpu-bench` で改善前と改善後を測り、「計測記録」に残す

# 0.9.0 の計画: C mruby の実行状態を続きから動かす機能を入れて出す

作成 2026-11-03。著者の判断（2026-11-03）:

1. ブランチ `feature/checkpoint` を**丸ごと**入れる（C mruby が mruby-checkpoint で書いた実行状態を続きから動かす、形式 v1。
   それと `041e096`: CLI がプログラムの出力を書かれたときに出す、`Vm::output_sink`）。merge commit で、ブランチの履歴を残す。
2. `sabiruby-cli` では feature `checkpoint` を**既定で on** にする（`cargo install sabiruby-cli` で `sabiruby --resume` が使える）。
   VM の crate `sabiruby` は既定で off のまま。CLI が gem を既定で on にする先例は `utf8`・`regexp`（`cli/Cargo.toml`）と、
   `posix-gems-plan.md` の著者の判断 2（CLI では既定で有効）。
3. `sabiruby` と `sabiruby-cli` は 0.9.0。ほかの crate を出すかは、前の公開からの変更を見て決める（下の表）。

## 状況（2026-11-03、`origin/main` `8b33beb`）

- 前のリリース: 0.8.0（2026-10-19、tag `v0.8.0`、`7f74cd1`）。`v0.8.0..origin/main` は 6 コミットで、すべて文書
  （0.8 の公開の記録、`docs/compare/mrubyedge.html` の 0.8.0 への更新、`regexp-engine-plan.md` と `posix-gems-plan.md` の案と
  著者の判断）。crate の中身（`src/`・`compiler/`・`cli/`・`serde/`・`macros/`）の差は無い。
- `feature/checkpoint`（`7fb1272` から分かれた 7 コミット、`79b80dc`〜`041e096`）: `src/checkpoint/`（5 ファイル）、
  `src/builtins/ext_task.rs` と `src/vm.rs` の cfg 付きの関数、`tests/checkpoint.rs` とイメージ 95 個、`cli/src/main.rs`
  （`--resume`、`--checkpoint-names`、出力の sink）、`Vm::output_sink`。
- 公開 API の変更は**足すものだけ**（`git diff v0.8.0.. -- src | grep -E '^[-+]\s*pub '` の `-` 側は空）。
  `Vm::output_sink` は `pub` のフィールドだが、`Vm` には非公開のフィールドがあり、ホストが構造体のリテラルで作ることは
  できないので壊さない。feature `checkpoint` の下に `Vm::checkpoint_*` の 4 つと `checkpoint::image_counts`・`data_types`。
  `src/checkpoint/` の中の `pub` の型（`image.rs`・`build.rs`）は非公開のモジュールの中なので外から見えない。
  壊さないので 0.8.1 でも semver は満たすが、機能が増えるリリースなので著者の判断のとおり minor を上げる。

## バージョン

| crate | 今 | 次 | 理由 |
|---|---|---|---|
| `sabiruby` | 0.8.0 | **0.9.0** | 著者の判断。feature `checkpoint`、`Vm::output_sink` |
| `sabiruby-compiler` | 0.5.0 | **0.6.0** | `git diff v0.8.0.. -- compiler/` は空。だが `host` で `sabiruby` を公開の依存に持つ（`impl sabiruby::Host for Compiler`）。0.5.0 のままだと CLI が 0.9 と 0.8 の VM を 2 つ抱え、`Compiler` を 0.9 の `Vm::set_host` に渡せない。0.x の caret では minor が major なので 0.6.0（`docs/design/compiler.md` "Publishing" の 0.6.0 の理屈） |
| `sabiruby-serde` | 0.4.0 | **0.5.0** | `git diff v0.8.0.. -- serde/` は空。`sabiruby` を公開の依存に持つので、0.9 の VM と使うには 0.9 を指す版が要る（0.8 の serde と同じ理屈） |
| `sabiruby-cli` | 0.8.0 | **0.9.0** | 著者の判断。`--resume` を既定で、出力の出し方 |
| `sabiruby-macros` | 0.1.1 | 0.1.1 のまま | `git diff v0.8.0.. -- macros/` は空。VM に依存しない |

## 段階

| 段階 | 中身 | 担当 |
|---|---|---|
| R1 | 取り込み。worktree `release-0.9`（`origin/main` から）に `feature/checkpoint` を merge commit で入れる | 実装担当 |
| R2 | 整理。`cli/Cargo.toml` の `default` に `checkpoint`、バージョンの番号と crate 間の要求、README（Status の 1 項目、コマンドの一覧、Usage の番号）・`cli/README.md`・`macros/README.md`・`docs/design/compiler.md` "Publishing" の 1 段落・`docs/numbers.md`（CLI の出力の塊 64 KiB）・`docs/README.md` の索引。CHANGELOG を `## 0.9.0 — (unreleased; …)` で切る（日付は R5） | 実装担当 |
| R3 | 確かめ。`cargo test --workspace`（debug・release）、checkpoint off（workspace から CLI を外す、CLI を `--no-default-features`）、VM を feature ごと、custom 3 ビルド、`SABIRUBY_GC_STRESS=1` の `tests/checkpoint.rs`、`tools/check_no_std.sh`（checkpoint の有無）、wasm、CI の `cargo doc` 2 段（0.8 の R4 の教訓）、mrbtest 3 ビルド（Docker）、`cargo publish --dry-run`（依存の順）、下流（rubevy・playground の写し） | 実装担当 |
| R4 | ベンチ。`v0.8.0` と候補を `tools/bench_ab.sh` で。出力の経路（`Vm::write_out` の分岐）が増えただけなので、命令の数が同じかを先に見る。**著者の合図を待つ**（同じ PC で他の担当が動くので） | 実装担当（著者の合図の後） |
| R5 | 公開: CHANGELOG の日付と「To publish:」→「Published:」、`docs/design/compiler.md` の「to be published」を直して commit、`release-0.9` を main に入れて push、CI が通るのを見る、`cargo publish`（`sabiruby` → `sabiruby-compiler` → `sabiruby-serde` → `sabiruby-cli`）、tag `v0.9.0` | **著者の確認の後に**（外に出す操作） |
| R6 | 下流のバージョン上げ（rubevy 0.4 など、playground の `SABIRUBY_REF`、games）と Pages。各 repo の計画として別に立てる | 著者の判断の後 |

## 守ること

- VM の crate に `unsafe` を入れない。`no_std + alloc`、`Vm: Send + Sync`。
- 取り込みと番号のほかに挙動は変えない（コードの直しが要ると分かったら、止めて報告する）。
- 下流の確かめは他の repo にファイルを作らない（スクラッチパッドの写しで）。
- push・publish・tag・Pages は R5・R6 で、著者の確認の後。`release-0.9` はコードを含むので、文書だけの push の許可にも当たらない（push しない）。

## 状況

| 段階 | 状況 |
|---|---|
| R1 | **済み（2026-11-03）**。衝突は `docs/README.md` の計画書の表の 1 か所だけ（両方の行を残した） |
| R2 | **済み（2026-11-03）** |
| R3 | **済み（2026-11-03）**。数は worklog `2026-11-03-release-0.9.md` |
| R4 | **済み（2026-11-03）**。`bench/` の 27 本で命令の数と GC の回数が `v0.8.0` と全部同じ。時間は 1 回り（PC は空いていなかった）で全体 +0.4%、どれも ±4.0% の内（`docs/verification/bench.md` の 0.9.0 の節）。作業中に、`Checkpoint.save` / `suspend` が C の側の `source:` を受けずに ArgumentError になるのを見つけて直した（`9542961`、試験 `save_takes_the_source_keyword`） |
| R5 | **済み（2026-11-03）**。`a3a798d` を main に push、CI 緑（ubuntu・macOS・msrv）。crates.io に `sabiruby` 0.9.0、`sabiruby-compiler` 0.6.0、`sabiruby-serde` 0.5.0、`sabiruby-cli` 0.9.0、tag `v0.9.0`（`a3a798d`） |
| R6 | **済み（2026-11-03）**。rubevy の crates.io への公開 (`rubevy` 0.4.0・`rubevy-build` 0.4.0) と tag `v0.4.0` は著者が打った。rubevy `341db82`（0.4.0・rubevy-build 0.4.0、sabiruby 0.9。push 済み、`cargo publish` は手元の権限の設定で止められた）、playground `0e93e4e`（`SABIRUBY_REF` を v0.9.0 に）、games `79edd0c`（rubevy 0.4.0 の rev、sabiruby 0.9.0 が 1 つ、`PLAYGROUND_REF` を `0e93e4e` に）。両方の Pages のデプロイは成功。あわせて `feature/checkpoint` を消し、CI に checkpoint off の 2 段、`tools/mrbtest.sh` の直し（worklog `2026-11-03-release-0.9.md` の「R6」） |

# ブロックの内側でも待てるようにする（計画）

作成 2026-09-26。著者の判断（2026-09-26）: 「いつでも sleep できるように VM を直す方向でいきたい」。段階は 1 と 2 をやる（段階 3 は後で決める）。「通常時の性能の劣化がないようにしたい」。

## 背景

task は、`Task::Queue#pop`・`sleep`・`Task.pass` で止まって、他の task に譲る。組み込み先のスクリプト（ホストの答えを待つ逐次の DSL）では、DSL の block の中で待ちたい場面がよく出る。

- **今の SabiRuby では、ネイティブで書いた組み込みが block や Ruby のメソッドを呼び戻す経路の内側で待てない。**
  - 呼び戻しは `Vm::call_block` 系（`src/vm.rs:2135` ほか、33 か所）と `funcall`（約 100 か所）で、VM の実行ループを入れ子で起こす。入れ子の中から task を外へ出す道が無いので、そこが境界になる。
  - `pop` は `blocking pop cannot be called from within a C function boundary` を投げる。
  - 境界の内側の `sleep(n)` は、例外にならず**待たずに戻る**（0 フレーム）。
- **本家 mruby 4.1.0-rc2 の振る舞い**（2026-09-26 に 36 経路で実測。記録は book の `docs/notes/mruby-task.md`「ブロックの内側で待つ」）:
  - `instance_exec`・`instance_eval`・`class_exec`・`class_eval`・`module_eval`・`Method#call`・`bind_call`・`send` は境界に**ならない**。VM から呼ばれたときは block を自分で走らせず、呼び出し元の ci を書き換えて VM に返す（`vm.c` の `exec_irep` / `eval_under`、`mruby-method` の `mcall`、`send_method`）。
  - 境界になるのは、C から `mrb_yield` 系で block を呼ぶメソッドだけ。例: `Array#index {}`、`Array.new(n) {}`、`sort {}`、`Class.new {}`、`Module.new {}`、`send` が `method_missing` に落ちる場合、`ObjectSpace.each_object {}`。
  - 境界の内側では、`pop`・引数なしの `sleep`・`Task.pass` は `RuntimeError`。`sleep(n)` は壁時計で眠って全 task を止め、眠った後に `switching` を下ろすので、**その task は以後、横取りされなくなる**（本家の不具合の候補）。
- **SabiRuby と本家の違い**: `instance_exec`・`instance_eval`・`Method#call` の中で、本家は待てて SabiRuby は待てない。組み込み先の DSL は、これを避けるために handler を `define_method` でメソッドにしている。

## 到達点

**段階 1（本家と同じにする）**
- `instance_exec`・`instance_eval`・`class_exec`・`class_eval`・`module_exec`・`module_eval`・`Method#call`・`UnboundMethod#bind_call` を、VM から呼ばれたときは**入れ子にせず、呼び出し元のフレームを書き換えて VM に返す**形にする。`send` の `op_send_redirect`（`src/vm.rs:2313`）と同じ種類の仕組み。
  - ホスト（Rust）から `funcall` で呼ばれたときは、今までどおり入れ子でよい（本家も `cci` を見て分けている）。
- **境界の内側の `sleep(n)` / `usleep` / `sleep_ms` は `RuntimeError` にする。** 引数なしの `sleep` と `pop` に揃える。本家（壁時計で全体を止める）とは違う振る舞いになるので、選んだ理由を `docs/design/` に書く。
  - 理由: どちらの振る舞いも黙って別のことをしていて、task が 1 本の確認では気づけない。

**段階 2（DSL でよく使う block 付きの組み込みで待てるようにする。本家を超える）**
- 対象（まず読んで一覧を確定する。少なくとも次のもの）:
  - `Array#index {}` / `find_index {}`、`Array.new(n) {}`、`Array#sort {}` / `sort_by`、`Array#select!` などの破壊的な block 付き
  - `Hash.new {}`（default proc）
  - `Class.new {}` / `Module.new {}`
  - `send` / `public_send` が `method_missing` に落ちる場合
  - `Kernel#loop`、`Integer#times` などが Ruby で書かれているかも確かめる（本家 4.1.0 では `each`・`times`・`map`・`loop`・`tap` は mrblib の Ruby）
- 直し方は 1 つずつ選ぶ。
  - (a) 本家の mrblib と同じく、Ruby の前置きに移す。
  - (b) 段階 1 と同じく、フレームを書き換える形にする（block を呼んだ後に続きの処理があるものは、続きを Ruby の小さなメソッドにする、など）。
  - **速さの基準で決める。** Ruby に移して遅くなるものは (b) にする。
- **残る境界は、何の中の何の呼び戻しかを言う例外にする。** 例: `can't wait inside Array#join's call to #to_s`。黙った振る舞いは残さない。

**段階 3（しない。後で決める）**: 暗黙の呼び戻し（`join` / `inspect` の中の `to_s`、`include?` の `==`、`Hash` の `hash` / `eql?`、`sort` の `<=>`、ホストの関数からの呼び戻し）でも待てるようにすること。ネイティブのフレームを再開できる形（状態機械、または `core::future`）にする必要がある。段階 2 の後に、要るかを著者と決める。

## 守ること

- **VM の crate に `unsafe` を入れない**（`/home/kishima/book/CLAUDE.md`）。`no_std + alloc`（`tools/check_no_std.sh`）、`Vm: Send + Sync`（`tests/send_sync.rs`）を保つ。
- **本家テストの基準（`tests/mrbtest/baseline*.txt`）を下回らない。** 全ファイルを回して確かめる。
- **通常時の性能を落とさない**（著者の指定）。
  - 対象は `tools/bench.sh` の全ベンチと、段階ごとに触った経路のマイクロベンチ。例: `instance_eval` / `instance_exec` / `Method#call` / `send` の呼び出し、`sort {}`・`index {}`・`Array.new {}` の繰り返し、task の無い普通のループ。
  - 測り方: 変更前後を**交互に 2 巡以上**、best of 5（`tools/bench_ab.sh`）。別の `CARGO_TARGET_DIR` で建て、`md5sum` で 2 つが別のバイナリであることを確かめる。
  - **「劣化なし」の線は、先に同じバイナリどうしの A/A で測ったばらつきの幅から決める**（数を先に決めない）。その幅を超えて遅くなった経路は、直し方を変えるか、理由を書いて著者に出す。
  - 速くなったものも書く。入れ子の実行ループを起こさなくなるので、段階 1 の経路は速くなる見込み（見込みであって、測るまで書かない）。
- mruby の実装の選択（`cci` による分け方、末尾呼び出しの形）を写すときは、どこを写したかを `ファイル:行` で書く。

## 確かめ

- 新しいテスト（`tests/`）: task の中で、各経路の block の内側から `pop`・`sleep(n)`・`sleep`・`Task.pass` を呼ぶ。他の task の印で「本当に待ったか」を判定する（book の `data/code/vm_task_wait_in_blocks.rb` と同じ判定。SabiRuby 用に写してよい）。段階ごとに期待を「待つ」か「名指しの例外」に固定する。
- 本家との違いの表（`docs/design/` の新しい文書）: 経路ごとに、本家 4.1.0-rc2 / SabiRuby 変更前 / 変更後。
- rubevy から使ったときの確認: rubevy の queue（`Rubevy.ask(...).pop`）で同じ経路を 1 本ずつ試す。rubevy の repo は変えずに、SabiRuby のテストの中に最小の host を書いてよい。

## 段階

| 段階 | 到達点 | 確認 |
|---|---|---|
| W0 | 読む: 呼び戻しの全箇所の一覧（`call_block` 系 33、`funcall` 約 100）を「block を呼ぶ／暗黙の呼び戻し」に分け、段階 1・2 の対象を確定。A/A のばらつきを測って「劣化なし」の線を決める | 一覧と線を worklog に |
| W1 | 段階 1 | 新しいテスト、mrbtest の baseline、ベンチの前後、`check_no_std`、`send_sync` |
| W2 | 段階 2 | 同上 |
| W3 | 文書: 本家との違いの表、`sleep(n)` を例外にした理由、残る境界の一覧、CHANGELOG | — |

## 著者に後で聞くこと

- 段階 3 をやるか。
- バージョンを上げるか（0.6.x → 0.7.0。挙動が変わる: `sleep(n)` が例外になる、待てる経路が増える）。crates.io への公開は著者の判断。

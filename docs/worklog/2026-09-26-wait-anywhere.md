# 2026-09-26 ブロックの内側で待つ — フレームの書き換えと、名指しの境界

ブランチ `wait-anywhere`（main `3170b63` から）。計画は
[`../plans/wait-anywhere-plan.md`](../plans/wait-anywhere-plan.md)（W0〜W3。段階 1 と 2 をやり、段階 3 はしない）。
著者の指定は「通常時の性能の劣化がないように」。根拠の資料は book の `docs/notes/mruby-task.md`「ブロックの内側で待つ」
（本家 4.1.0-rc2 の 36 経路の実測）と `data/code/vm_task_wait_in_blocks.rb`。本家のソースは `../ref/mruby` を
`4.1.0-rc2`（`c17ffcc24`）で読んだ（作業の前から detached でそこにあり、`git checkout 4.1.0-rc2` は何も変えなかった）。

**性能の計測はまだ取っていない。** 作業の間、機械では著者の重い機械学習の計算が走っていて、その間の時間は意味が
無いので、A/A も前後の比較も後で取る（本体の指示、2026-09-26）。前のバイナリ（`3170b63`）は別の
`CARGO_TARGET_DIR` に建ててある。下の「劣化の線」と「ベンチ」の節は、計測のあとで埋める。

## W0: 呼び戻しの一覧と分類

### 境界とは何か（SabiRuby の今）

SabiRuby の「ネイティブの境界」は、今の文脈の `ci` に `Cci::Skip` のフレームがあること
（`Vm::fiber_check_native`、`src/vm.rs`）。`Cci::Skip` を積むのは `call_proc_inner` だけで、
ネイティブが block や Ruby のメソッドを呼び戻すとき（`call_block` 系と `funcall` → `call_proc_with`）に、
入れ子の `run_loop` をホストのスタックに起こす。その内側から task を外へ出す道は無い（Rust のフレームを
残して切り替えられない）ので、`Task::Queue#pop`（`queue_pop_try`）・引数なしの `sleep`・`Task.pass` は
例外を投げ、`sleep(n)` は `sleep_us` が「task でない」側に落ちて **待たずに戻っていた**
（`current_task(vm).filter(|_| !fiber_check_native)` が None になり、`sleep_hook` も無ければ時計を進めるだけ）。

本家の境界は `ci->cci > CINFO_NONE`（`src/vm.c:804-807`）。違いは、本家は `instance_exec` などを
**VM から呼ばれたときは block を自分で走らせず、呼び出し元の ci を書き換えて VM に返す**こと
（`exec_irep`、`src/vm.c:1601-1633`。`mrb_exec_irep` が `ci->cci == CINFO_NONE` で分ける、1635-1663）。

### 一覧

`call_block` 系を呼ぶ箇所（`grep -n "call_block\w*(\|call_proc(\|call_method_proc(\|run_eval("`、定義と文書を除く）と、
呼び出し元の block をそのまま渡す `funcall`、その他の `funcall`（約 95 か所）に分けた。
「影」は、同じ名前を mrblib（本家の Ruby 部分）が後から定義し直していて、ネイティブが呼ばれないもの。
これは probe（下記）で実際に待てることを確かめて判定した（`c_count`、`c_ary_fetch` などが「待つ」になる）。

**block を呼ぶ（呼び出し元が渡した block）**

| 箇所 | メソッド | 扱い |
|---|---|---|
| `ext_object.rs:26` | `instance_exec` | 段階 1 |
| `ext_class.rs:68` | `class_exec` / `module_exec` | 段階 1 |
| `object.rs:38,146,147`、`ext_eval.rs:198,216` | `instance_eval` / `module_eval` / `class_eval`（block） | 段階 1 |
| `ext_eval.rs:141,178,209,224` | `eval`、`Binding#eval`、`instance_eval` / `class_eval`（文字列） | 段階 1（本家の `eval_irep` も `mrb_exec_irep`、`mruby-eval/src/eval.c:153`） |
| `ext_eval.rs:107` | `top_load`（テスト用の `__backref_nested_load`） | 入れ子のまま。ホストの `mrb_load_string` を真似る関数なので |
| `ext_method.rs:104` | `Method#call` / `UnboundMethod#bind_call` | 段階 1（`mcall` → `mrb_exec_irep`、`mruby-method/src/method.c:278`） |
| `array.rs:141` | `Array.new(n) {}`（`Array#initialize`） | 段階 2 |
| `array.rs:87` | `sort! {}`（`sort {}` は mrblib が `dup.sort!` で書く） | 段階 2 |
| `array.rs:193` / `:202` | `index {}` / `rindex {}` | 段階 2 |
| `array.rs:212` | `delete(x) {}`（見つからないとき） | 段階 2 |
| `hash.rs:62` | 既定の proc（`Hash.new {}` の `h[k]`） | 段階 2 |
| `hash.rs:153` | `Hash#default(k)`（proc があるとき） | 段階 2 |
| `object.rs:218,224` | `Class.new {}` / `Module.new {}` | 段階 2 |
| `ext_struct.rs:214` / `ext_data.rs:145` | `Struct.new {}` / `Data.define {}` | 段階 2 |
| `ext_catch.rs:32` | `catch {}` | 段階 2（本家の `catch` はバイトコード、`mruby-catch/src/catch.c:20-45`） |
| `ext_objectspace.rs:97` | `ObjectSpace.each_object {}` | 境界のまま。本家も境界（GC が走査中） |
| `ext_regexp.rs:605,846,1418`、`string.rs:876` | `sub` / `gsub` / `scan` / `Regexp#match`（block） | 境界のまま（名指しの例外）。文字列の処理の途中で待つ用途が見当たらない |
| `array.rs:150,156,188,213-216,225` | `count` `fetch` `to_h` `delete_if` `reject!` `select!` `keep_if` `sort_by` | 影（mrblib の Ruby）。すでに待てる |
| `hash.rs:135,150,182-184` | `fetch` `delete` `merge` `merge!` `update` | 影 |
| `string.rs:584,585,646` | `each_char` `each_byte` `upto` | 影 |
| `proc_.rs:29` | `proc_call` | Ruby からは呼ばれない（`Proc#call` は `OP_CALL` 1 命令のメソッド） |
| `mrbtest.rs:96` | `c_tunnel`（fiber のテスト用） | 境界のまま（境界を作るための関数） |

**呼び出し元の block をそのまま渡す `funcall`**

| 箇所 | メソッド | 扱い |
|---|---|---|
| `object.rs:194` | `Class#new` → `initialize` | 段階 1。本家の `new` はバイトコード（`new_iseq`、`src/class.c:4596-4631`）で、`initialize` は普通のフレーム |
| `object.rs:64`、`ext_metaprog.rs:138` | `public_send` | 段階 1（本家は `send_method(pub=TRUE)`、`src/vm.c:1688`） |
| `object.rs:444` | `send`（ネイティブから呼ばれたとき） | 段階 1 と同じ形にそろえる |
| `ext_method.rs:101,110` | `Method#call`（`method_missing` 行き、ネイティブ以外） | 段階 1 と同じ形にそろえる |
| `ext_struct.rs:280,284`、`proc_.rs:65` | Struct の `new` → `initialize`、`Proc.new` → `initialize` | 境界のまま（名指し） |
| `mod.rs:185` | `Vm::class_new_instance`（ホストの API） | 入れ子のまま |

**暗黙の呼び戻し（段階 3。境界のまま、名指しの例外にする）**: `to_s` / `inspect`（`join`、`p`、式展開の外）、
`==` / `eql?` / `hash`（`include?`、Hash の鍵）、`<=>`（`sort` の block なし、`max`）、`to_ary` / `to_str` / `to_proc` /
`to_a` / `to_i`、`coerce`、`respond_to_missing?`、ネイティブの `method_missing` の代わり、`const_missing`、
`inherited` / `method_added` などのフック、`allocate`、`sum` の `+`、`dig` の `[]`、`===`、`=~`、`exception`、
`initialize_copy`、ホストの関数（`define_closure`）からの `funcall`。

### probe

`tests/wait/probe.rb` は book の `vm_task_wait_in_blocks.rb` を SabiRuby 用に写したもの。本家の 36 経路に、
SabiRuby で分かれ目になるもの（`new` → `initialize`、`eval` の文字列、`public_send`、`catch`、Hash の既定の proc、
`Struct.new {}`、正規表現の block、暗黙の呼び戻し 2 つなど）を足して 64 経路。待ち方は `pop`・`sleep 0.1`・
`sleep`（引数なし）・`Task.pass`・`usleep`・`sleep_ms` の 6 つと、ホストが答えるキュー（`host`）。
判定は本家のときと同じ（`main` の start と end の間に `other` の印があれば待った）。
`tests/wait_anywhere.rs` が 64 × 7 を 1 つずつ別の VM で回し、段階ごとの期待と比べる。`busy`（タイマーによる切り替え）は
入れていない。SabiRuby の持ち時間は命令数で、今回の変更の対象ではない。

変更前（`3170b63`）の結果（CLI で 64 × 6、`$SP/matrix.sh`）: 待てるのは本家の「待つ」のうち
`instance_exec` 系・`Method#call` 系を除いたもの。`instance_exec`・`instance_eval`・`class_exec`・`class_eval`・
`module_eval`・`Method#call`（block あり・なし）・`bind_call`・`public_send`・`new` → `initialize`・`eval` の文字列は
`pop`・`sleep`・`Task.pass` が例外、**`sleep(n)`・`usleep`・`sleep_ms` は「待たずに戻る」**（`did not wait`）。
`send` → `method_missing` は本家では境界だが SabiRuby ではすでに待てる（`op_send_vis` が Ruby の `method_missing` を
同じフレームで呼ぶ、`docs/design/fibers.md`）。

## W1: 段階 1

### 仕組み

本家の `exec_irep` と同じことを、SabiRuby のフレームの形でやる。SabiRuby のネイティブはフレームを持たない
（SEND がネイティブを呼ぶとき ci を積まない）ので、「今の ci を書き換える」ではなく
**「SEND の結果が入るレジスタ（`native_ret_reg`）を R0 にしたフレームを 1 つ積んで、ネイティブは R0 の値を返す」**
になる。ネイティブが返ると `op_send_vis` がその値を `R[a]` に書くが、それは積んだフレームの R0 と同じ値なので壊れない。
命令のループは次の命令を一番上のフレーム（積んだもの）から読む。そのフレームは `Cci::None` なので境界にならず、
返るときは普通の `OP_RETURN` と同じく呼び出し元の `R[a]` に値を置く。

- `Vm::in_frame()` = `direct_send`。「今のネイティブは、今のフレームの SEND から呼ばれた」。本家の `ci->cci == CINFO_NONE`。
- `Vm::exec_proc`（`mrb_exec_irep`）: `in_frame` ならフレームを積んで `self_` を返す。そうでなければ今までどおり
  `call_proc_with`（入れ子）。`target_class` と `mid` の決め方は `call_proc_inner` と同じにして、違いを境界だけにした。
- `Vm::push_frame_at`: 引数の並べ方は `relay_args`（`send` のやり直しと同じ）。
- `exec_block_with_self`（`instance_exec` / `class_exec` / `instance_eval` / `class_eval` / `module_eval`）、
  `exec_method_proc`（`Method#call`）、`run_eval`（文字列の `eval` 系）がこれを使う。
- `Vm::send_in_frame`（`send_method`）: `public_send`・ネイティブから呼ばれた `send`・`Method#call` の残りの枝。
  Ruby のメソッドと Ruby の `method_missing` はフレームを積み、ネイティブは `direct_send` を保ったまま直接呼ぶ
  （`public_send(:instance_exec) {}` の内側も待てる）。
- `Class#new`: `initialize` が Ruby なら、**`OP_RETURN R0` 1 命令の小さなフレーム**（`ret_proc`、ireps[1]）を先に積み、
  その上に `initialize` を積む（`Vm::push_return_frame`）。`initialize` の値は小さなフレームの R1 に入り、
  小さなフレームが R0（= 作ったオブジェクト）を返す。本家の `new_iseq`（`SSENDB :initialize` の後 `RETURN R0`）の
  後半をそのまま 1 フレームにしたもの。Ruby の `new` を書く（splat と kwargs のハッシュを作る）より軽いと見込んだ
  （見込み。計測で確かめる）。

### `direct_send` を正確にする

`direct_send` は今まで Fiber の `resume` / `yield` / `transfer` が「入れ子のループが要るか」を決めるのにだけ使っていて、
**入れ子のループの中でも外側の値が残っていた**（`call_block` は `direct_send` を下ろさない）。そのままだと、
外側のネイティブ X が SEND から呼ばれて block を呼び、その block の中の `h[k]`（`OP_GETIDX` がネイティブの
`hash_aref` を SEND を通さずに直接呼ぶ）が「SEND から呼ばれた」と見え、X の `native_ret_reg`（下のフレームの
レジスタ）にフレームを積んでしまう。`run_loop_ctx` の入り口で下ろし、出口で戻すことにした（1 か所）。
SEND がネイティブを呼ぶ `call_native_direct` は今までどおり立てるので、Fiber の判定は変わらない。

`exec_proc` と `push_method_frame` は、積んだ後に `direct_send` を下ろす。同じネイティブが 2 つ目を積むと、1 つ目の
フレームの上に重なってしまうので（二重に使う呼び出しは無いが、使い方を誤ったときに壊れ方を小さくする）。

### `sleep(n)` を例外に

`sleep_us` は、task の中で境界の内側なら `RuntimeError` を投げる（`sleep`・`usleep`・`sleep_ms`）。
本家は壁時計で眠って全 task を止め、眠った後に `switching_` を下ろすので、その task は以後タイマーで切り替わらなく
なる（book の `mruby-task.md`「C の境界の内側の sleep(n) は、持ち時間切れの予約を消す」）。SabiRuby の変更前は
待たずに戻っていた。どちらも task が 1 本の確認では「待てた」ように見えるので、黙った振る舞いをやめて例外にした。
task の外（ルートの文脈）の `sleep(n)` は今までどおりホストの `sleep_hook` で眠る。

### 境界の名指し

4 つの待ち方（`pop`・`sleep`・`sleep(n)` 系・`Task.pass`）の例外を、どのネイティブの何の呼び戻しかを言う形にした:

```
can't wait inside Array#join's call to #to_s (Task::Queue#pop)
can't wait inside Array.new's call to a block (sleep)
```

名前はホットパスに何も足さずに、**例外を作るときに**割り出す（`Vm::boundary_name`）。一番内側の `Cci::Skip` の
フレームが「呼び戻された側」（`mid` があればメソッド、無ければ block）。その下のフレームは、境界を作った
ネイティブを呼んだフレームで、その `pc` はまだ呼び出しの命令の直後を指し、受け手はまだ `R[a]` に残っている
（ネイティブの結果は返った後で書かれる）。そこで irep を頭から読み直して `pc` の直前の命令を見つけ
（`Vm::send_before`、EXT の前置きも数える）、SEND の名前と受け手のクラス（メソッドの持ち主。モジュールなら
モジュール、クラスが受け手なら `Array.new` の形）を並べる。`send` のやり直しや `super` からの場合は名前がずれるか
「a native method」になるが、例外の文言の話なので割り切った。

最初は CallInfo にネイティブの名前を持たせることを考えたが、CallInfo は今 56 バイトで、`Option<Sym>` を足すと
64 バイトになり、すべての SEND の push が重くなる。Vec の横持ちも、呼び戻しのたびに push/pop が要る。
例外を作るときに読み直す形なら、通常の実行は何も払わない。

本家の文言（`blocking pop cannot be called from within a C function boundary` など）は使わなくなった。
本家のテストにも SabiRuby のテストにも、この文言を比べているものは無かった（`grep -rn boundary`）。

### 確かめたこと（W1）

- probe（CLI、変更前 → W1）: 本家で「待つ」経路のうち、変更前に待てなかった
  `instance_exec`・`instance_eval`・`class_exec`・`class_eval`・`module_eval`・`Method#call`（2 種）・`bind_call` と、
  本家と同じ形にした `public_send`・`new` → `initialize`・`new` の block・`eval` の文字列・`instance_eval` の文字列が、
  6 つの待ち方すべてで「待つ」になった。残る境界は全部、名指しの `RuntimeError`（`did not wait` は 0 件）。
- 意味の違いを探した小さなスクリプト（戻り値、`break`、`return`、例外と backtrace、kwargs、16 引数、
  特異メソッドの定義、`__method__`、`caller`）を前後のバイナリで比べた。違いは 1 つ:
  **`Foo.new { break 9 }`（`initialize` が yield する）が、前は `Foo` のインスタンス、今は `9`**。
  block の `break` は block を受け取ったメソッド（`new`）から抜けるので、今のほうが CRuby と本家（`new` がバイトコード）
  に合う。
- `instance_exec` を入れ子にするような深い再帰は、前は `NATIVE_DEPTH_MAX`（96）で `SystemStackError`、今は
  `CALL_LEVEL_MAX`（512 フレーム）で止まる。本家も ci の深さで止まる。
- mrbtest（本家のテスト、全 108 ファイル）の通過数は前後で 1 件も変わらない（`$SP/mrbt.sh` で前後の CLI を比べた）。
  `cargo test --release` 全部、bytes と no-regexp の構成の `mrbtest` と `wait_anywhere`、`tools/check_no_std.sh` が通る。
- 最初の実行で `object ObjId(44) is not a proc` で落ちた。足した `ret_proc` を GC の根に入れていなかった
  （`call_proc` は `gc_mark_roots` で印を付けている）。1 行足して直した。

## W2: 段階 2

### 対象の確定

W0 の表の「段階 2」の行。probe で確かめると、計画に挙げた `select!` などの破壊的な block 付き・`sort_by`・
`count`・`to_h`・`fetch` と、Hash の `fetch`・`delete`・`merge` / `update`、String の `each_char` / `each_byte` /
`upto`、`Integer#times`・`Kernel#loop` は **mrblib の Ruby が同じ名前を後から定義していて、ネイティブは呼ばれていない**
（W1 の時点で全部「待つ」）。残ったのは次の 13 経路:
`Array#index {}`・`rindex {}`・`Array.new(n) {}`・`sort {}`（mrblib の `sort` が `dup.sort!` を呼ぶので実体は `sort!`）・
`Array#delete(x) {}`・`Hash.new {}` の `h[k]`・`Hash#default(k)`・`Class.new {}`・`Module.new {}`・`Struct.new {}`・
`Data.define {}`・`catch {}`・`Regexp#match {}`（と、それを呼ぶ `String#match {}`）。

`send` / `public_send` が `method_missing` に落ちる場合は、SabiRuby では W1 の前から同じフレームで呼んでいた
（`op_send_vis`、`docs/design/fibers.md`）。W1 で `public_send` も同じ形にした。

### 直し方（1 つずつ）

計画は「Ruby の前置き（a）か、フレームの書き換え（b）かを速さで選ぶ」。速さはまだ測れない（冒頭）ので、本体の指示どおり
**全部 (b) にして、速さの確認は計測待ちの印を付ける**。(b) の中で 3 つの形になった。

1. **block の値がそのまま答え**（続きの処理が無い）: `Array#delete(x) {}`（見つからないとき）・`Hash#default(k)`・
   `Hash#[]` の既定の proc・`Regexp#match {}`。`Vm::exec_block`（`call_block` のフレーム版）で W1 と同じ。
   `String#match {}` は `Regexp#match` を `send_in_frame` で呼ぶ。
2. **block の後に決まった値を返すだけ**: `Class.new {}`・`Module.new {}`・`Struct.new {}`・`Data.define {}`。
   W1 の `Class#new` と同じく、`OP_RETURN R0` の小さなフレームの上に block のフレームを積む
   （`Vm::exec_block_with_self_then`）。
3. **block を繰り返し呼ぶ**: `index`・`rindex`・`Array.new(n)`・`sort!`。続きは Ruby の小さなメソッドにした
   （計画の (b) の「続きを Ruby の小さなメソッドにする」）。`src/mrblib/block-frames.rb` に `__index_by`・
   `__rindex_by`・`__init_by`・`__sort_by_block!`（private）を置き、参照の `mrbc`（Docker `kishima/mruby:4.1.0-rc2`）で
   `block-frames.mrb` に焼いて埋め込む（`require.rb` と同じ扱い、`tools/fixtures.sh` に 1 行）。ネイティブは block があって
   `in_frame` のときだけそちらへ `send_in_frame` し、`funcall` から呼ばれたときや block の無いときは今までの Rust のまま。
   **block の無い呼び出しは何も変わらない**（`b.is_nil()` の判定は前からあり、足したのは `in_frame` の 1 回の読みだけ）。
   - 各メソッドはネイティブのループを 1 手ずつ写した。`rindex` は毎回長さを読み直す（block が配列を縮めうる）、
     `Array.new(n)` は要素を集めてから 1 度に入れる、`sort!` は同じボトムアップのマージソートで、同じ組を同じ順で比べる。
     比べた答えの読み方（Integer は符号、nil はエラー、それ以外は `> 0` / `< 0` を聞く）は `sort_order` として
     Rust に切り出し、ネイティブのソートと Ruby 側（`__sort_cmp`）の両方が使う。矛盾した block でも前と同じ順になることを
     テストで固定した（`[5, 4, 3, 2, 1, 0].sort { n += 1; n % 3 - 1 }` → `[0, 5, 4, 1, 3, 2]`、変更前のバイナリの答え）。
   - `Array.new(n) {}` は `Class#new` → `initialize`（ネイティブ）を通る。W1 の `Class#new` は Ruby の `initialize` だけを
     フレームにしていたので、**block があるときはネイティブの `initialize` もフレームを保ったまま呼ぶ**ようにした
     （`Vm::send_in_frame_then`: 答えのフレームを積んでから呼び、何も積まれなかったら答えのフレームを外す）。

`catch` だけは形が違う。本家の `catch` はバイトコードのメソッド（`catch_iseq`、`mruby-catch/src/catch.c:20-45`）で、
`throw` はフレームを下から見て、その proc で R1（タグ）が同じものを探す（`find_catcher`）。SabiRuby は `catch` を
ネイティブにして `Vm::catch_tags` にタグと深さを積んでいたが、フレームにすると「ネイティブが返ったら外す」ができない。
本家の 29 バイトの命令列をそのまま `VmIrep` にして `Kernel#catch` にし（`ext_catch.rs`）、`throw` を本家と同じ探し方に
した。`catch_tags` は要らなくなり、`Vm::catch_proc` に置き換えた（GC の根にも入れる）。

### `Hash#[]` と `OP_GETIDX`

`h[k]` はほとんど `OP_GETIDX` で、`op_getidx` がネイティブの `hash_aref` を SEND を通さずに直接呼んでいた。
直接の呼び出しは `in_frame` にならないので、既定の proc はそのままでは入れ子のまま。そこで `op_getidx` / `op_getidx0` で
**自分で引いて、無くて既定の proc があるときだけ send に回す**ようにした（send なら `Hash#[]` が `in_frame` で呼ばれる）。
既定の proc の有無は ivar の読み 1 回で、その名前（`__default_proc`）は `Syms` に 1 つ足した。
当たったときは前より仕事が減る（`call_native` の出入りと `argc!` が無くなる）。外れて既定の proc が無いときは、
引いた後の処理だけの `hash_missing` を呼ぶので、引き直しはしない。速さは計測待ち。

### 失敗と直したもの

- **`Hash#values_at` が壊れた**（mrbtest `gem_hash` 27 → 26）。`values_at` はネイティブの中から `hash_aref` を
  Rust の関数として呼び、答えを集めて先へ進む。`values_at` 自身が SEND から呼ばれているので `in_frame` が真のまま
  `hash_aref` に届き、最初の既定の proc でフレームを積んで戻ってしまった。**「SEND から呼ばれたネイティブが、その答えを
  そのまま返すときだけ」**という前提を、ネイティブを助け手として呼ぶ側が破る形。`hash_aref` を「そのまま返す入口」と
  「続きのある呼び手用の `hash_value`（既定の proc は入れ子）」に分けて直した。同じ形の呼び出しが他に無いかを、
  変えた関数を名前で全部 grep して確かめた（`mcall`・`str_match`・`eval_in_binding` などは呼び手が答えをそのまま返す）。
- **`tests/task.rs` の 2 つ**（`Task::Overrun`）が落ちた。`Array.new(1) { loop { } }` を「ネイティブの境界で止まって
  切り替えられない task」の例にしていたが、もう境界ではないので、普通に切り替わって Overrun にならない。
  例を残る境界（`Array#join` が呼ぶ `to_s` が終わらない）に替えた。テストの意図は変えていない。
- no_std の構成で `vec!` が無かった（`ext_catch.rs` に `use alloc::vec;`）。

### 意味の違い（W2）

前後のバイナリで同じスクリプト（`index`・`rindex`・`Array.new`・`sort`・`delete`・既定の proc・`values_at`・
`Class.new`・`Module.new`・`Struct.new`・`Data.define`・`catch` / `throw`（入れ子、深い再帰、`ensure`、見つからないタグ、
block なし、`Kernel.catch`、`Method#call` 経由）・`Regexp#match`）を比べた。違いは 3 つで、全部 `break`:

| 式 | 変更前 | 変更後 |
|---|---|---|
| `[1, 2, 3].index { break :b }` | `0` | `:b` |
| `Array.new(3) { \|i\| break :early if i == 1; i }` | `[0, :early, 2]` | `:early` |
| `Class.new { break :cb }` | そのクラス | `:cb` |

変更前は、入れ子のループの中の `break` が block の値として返り、ネイティブがそれを使って先へ進んでいた。
変更後は block を受け取ったメソッドから抜ける。Ruby の `break` の意味（CRuby の振る舞い）に合うのは変更後。
本家での振る舞いは今回は動かして確かめていない。

GC を毎回走らせる設定（`SABIRUBY_GC_STRESS=1`）でも、probe の主な経路・上の比較スクリプト・関係する mrbtest の
18 ファイルの通過数が変わらないことを確かめた。

## 気づいた点

- `Vm::funcall` の「メソッドが無い」枝で、クロージャの `method_missing` を `call_closure` で呼ぶとき
  `direct_send` を下ろしていない（`src/vm.rs` の `funcall`、`None =>` の `Method::Closure` の行）。ネイティブの枝は下ろす。
  そのクロージャが Fiber の `yield` を呼ぶと、入れ子なのに「SEND から呼ばれた」と見える。VM（sabiruby）の話。
  今回の変更では、`run_loop_ctx` が下ろすようになったので入れ子のループの中では起きないが、SEND から呼ばれたネイティブが
  `funcall` でこの枝に来た場合は残る。
- rubevy の `src/prelude.rb` の冒頭のコメントは、境界の例外の文言を
  `"blocking pop cannot be called from within a C function boundary"` と引いている。文言はこの変更で
  `can't wait inside ... (Task::Queue#pop)` になった。rubevy の文書と実物のずれ（rubevy の側で直す話。この repo からは触らない）。
- `in_frame` の前提（「SEND から呼ばれたネイティブが、その答えをそのまま返すときだけ」）は型では守られていない。
  ネイティブの関数を別のネイティブが助け手として呼ぶと破れる（`values_at` の件）。今は変えた関数の呼び手を
  grep で全部見たが、これから `exec_*` / `send_in_frame` を使う関数を足すときに同じ確認が要る。VM の話。

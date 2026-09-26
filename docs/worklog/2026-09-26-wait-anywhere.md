# 2026-09-26 ブロックの内側で待つ — フレームの書き換えと、名指しの境界

ブランチ `wait-anywhere`（main `3170b63` から）。計画は
[`../plans/wait-anywhere-plan.md`](../plans/wait-anywhere-plan.md)（W0〜W3。段階 1 と 2 をやり、段階 3 はしない）。
著者の指定は「通常時の性能の劣化がないように」。根拠の資料は book の `docs/notes/mruby-task.md`「ブロックの内側で待つ」
（本家 4.1.0-rc2 の 36 経路の実測）と `data/code/vm_task_wait_in_blocks.rb`。本家のソースは `../ref/mruby` を
`4.1.0-rc2`（`c17ffcc24`）で読んだ（作業の前から detached でそこにあり、`git checkout 4.1.0-rc2` は何も変えなかった）。

W0〜W3 のコードを書いている間、PC では著者の重い機械学習の計算が走っていて、その間の時間は意味が無いので、
計測は後に回した（本体の指示、2026-09-26）。PC が空いてから取った計測と、それで分かって直したこと（W2b〜W2c）は
末尾の「計測」の節。

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

## W3: 文書

- `docs/design/wait-anywhere.md`（新規、英語）: 仕組み、使う側の約束（「SEND から呼ばれたネイティブが、その答えを
  そのまま返すときだけ」）、本家 4.1.0-rc2 / 変更前 / 変更後の経路ごとの表、`sleep(n)` を例外にした理由、残る境界の一覧と
  例外の文言。表の本家の列は book の実測（36 経路）で、実測していない経路はソースを読んだもの（「source:」と書いた）か
  「not measured」。
- `docs/design/fibers.md` の "Native boundaries" に 1 段落（同じ動きの一般化と、上の文書への参照）。
- `docs/design/gc.md`: 「ネイティブの下で動く Ruby」の例に `sort { }` と `Class#new` → `initialize` を挙げていたのを直した
  （SEND から呼ばれたときはもうネイティブの下ではないので、GC も止まらない）。
- `src/vm.rs` の `RunLimits` の rustdoc も同じ例を挙げていたので、残る境界の例に替えた。
- `CHANGELOG.md` の Unreleased、`docs/README.md` の目次（design と worklog）。
- `docs/design/gems.md` の 2026-09-14 の記録（`Array.new(1) { loop { } }` が番を返さなかった）は、その日の計測の記録なので
  そのままにした。

## 計測の準備（W3 の時点）

PC が空くまで取らない（冒頭）。用意したもの:

- バイナリ（それぞれ別の worktree と `CARGO_TARGET_DIR` で建てた。md5 は 3 つとも別）:
  - 変更前 `3170b63`: `$SP/wa/target-before/release/sabiruby`（`c78f78a0…`）
  - W1 `9eaf698`: `$SP/wa/target-w1/release/sabiruby`（`b053a060…`）
  - W2 `da68a3a`: `$SP/wa/target-w2/release/sabiruby`（`2ae9d026…`）
  （`$SP` は作業者のスクラッチパッド。repo には置かない）
- マイクロベンチ 21 本（`$SP/wa/micro/`、参照の `mrbc` で焼いた）: task の無い普通のループ・メソッド呼び出し・`yield`、
  `instance_exec` / `instance_eval` / `class_exec` / `Method#call` / `send` / `public_send`、`new`（Ruby の `initialize` と
  `Object.new`）、`sort { }` と `sort`、`index { }` と `index(x)`、`Array.new(n) { }`、`h[k]` の当たり・外れ・既定の proc、
  `catch`、`Class.new { }`。`N` は命令数から決めた（1 本あたり 1〜3 千万命令の見当。重いネイティブのものは少なめ）。
- 手順: まず A/A（変更前どうし）を `tools/bench_ab.sh` の形（`$SP/wa/bench_ab_micro.sh` は `DIR` を変えられるようにした写し）で
  `bench/` 全部とマイクロベンチに取り、best の差の幅から「劣化なし」の線を決める。次に 変更前 ↔ W2 を交互に 2 巡以上
  （`--core 2`、best of 5）、`vmstat 1 5` を前後に。

## 計測（PC が空いてから、2026-09-26 12:40〜）

### 取り方

- バイナリはそれぞれ別の worktree と `CARGO_TARGET_DIR` で建て、md5 が別であることを確かめた
  （変更前 `3170b63` `c78f78a0…`、W1 `9eaf698` `b053a060…`、W2 `da68a3a` `2ae9d026…`、W2b `33cc076` `a7cc91e2…`、
  W2c `f3c7d9c` `dfb47da8…`、W2d `aa4b734` `94637be6…`）。
- `tools/bench_ab.sh`（27 本）と、同じ形でディレクトリだけ替えたマイクロベンチ 21 本。1 巡 = A と B を交互に 5 回ずつ、
  `--core 2`。巡の間で A と B の順を入れ替えた（名前が `r` で終わる巡。`bench_ab.sh` は毎回 A を先に走らせるので、
  後に走る側への偏りを打ち消すため。A/A の 1 巡目は B 側に平均 +0.8% ほど寄っていた）。
- 数の読み方: ベンチごとに、全巡を通した A の最速と B の最速（5 回 × 巡の数）を比べる。巡ごとの変化も並べる。
- 始める前に `vmstat 1 5`（idle 99%）と `pgrep`。**13:24 から、別の担当の C のビルドとテスト（`cc1` が 20 本前後、
  `build/test-results/*.c`）が断続的に走った。** 巡の前に idle 97% 以上を待つようにし、`vmstat -t 10` を記録し続けて、
  巡の間に idle 90% 未満の標本が出た巡は捨てるか、捨てた上で取り直した（どの巡が該当したかは下の各表に書く）。

### A/A と「劣化なし」の線

変更前どうしの A/A を、マイクロ 3 巡、`bench_ab.sh` 3 巡（12:42〜13:21 の分は別の担当が動き出す前、
3 巡目は 14:30〜 の静かな時間）。

- マイクロ: 全巡を通した最速どうしの差の最大は **2.7%**（`m_new_plain`）。巡ごとの値では最大 6.3%（`m_hash_miss`）。
- `bench_ab.sh`: 全巡を通した最速どうしの差の最大は **6.6%**（`gc_churn`）、次が 6.3%（`app_tak`）、その次は 4.1%。
  巡ごとの値では 56.6%（`vmo_index`、1 巡だけ遅い状態に入った）がある。

**線**: 全巡を通した差がその A/A の最大（マイクロ 2.7%、`bench_ab.sh` 6.6%）を超え、**かつ** 全巡で正の向きのもの
を「遅くなった」とする。線を A/A の最大にしたのは、この PC で同じバイナリどうしがそれだけ離れうることを
今日測ったからで、それより狭い線には根拠が無い。ただし線の内側でも、全巡で同じ向きに数 % 動くものは下で名指しする。

### 1 回目の結果（W1・W2）と、それで直したこと

マイクロ（全巡の最速どうし。W1・W2 は 2 巡）:

| ベンチ | A/A | W1 | W2（Ruby のループ） | W2b | W2d |
|---|---:|---:|---:|---:|---:|
| `m_array_new_block` | +0.3% | +2.2% | +149.2% | +1.6% | -3.5% |
| `m_catch` | -0.2% | +0.6% | +16.0% | +4.0% | +2.8% |
| `m_class_exec` | -1.9% | -5.0% | -6.6% | -6.1% | -6.1% |
| `m_class_new_block` | +0.5% | +0.6% | +8.8% | -2.8% | -1.2% |
| `m_hash_default_proc` | -1.1% | +4.4% | +28.9% | -16.7% | -25.6% |
| `m_hash_hit` | -0.5% | -1.3% | -5.6% | -6.0% | -4.4% |
| `m_hash_miss` | +1.9% | +1.0% | +8.6% | +9.9% | -21.1% |
| `m_index_arg` | +1.1% | +3.0% | +0.1% | +1.7% | +1.1% |
| `m_index_block` | -0.4% | +4.3% | +160.2% | -11.7% | -12.6% |
| `m_instance_eval` | -0.9% | +0.9% | -0.8% | -8.7% | -7.9% |
| `m_instance_exec` | +0.2% | -2.3% | -3.5% | -8.2% | -7.5% |
| `m_method_call` | -0.3% | +0.1% | -0.5% | -6.6% | -6.5% |
| `m_method_loop` | -1.2% | -1.2% | +1.3% | +2.5% | +3.0% |
| `m_new_plain` | -2.7% | +3.6% | -1.0% | +3.1% | -9.1% |
| `m_new_ruby_init` | -0.2% | +9.4% | +9.0% | -6.8% | -5.2% |
| `m_plain_loop` | +0.3% | +0.1% | +0.6% | +0.7% | +2.2% |
| `m_public_send` | +1.5% | +2.9% | -0.5% | -7.3% | -5.8% |
| `m_send` | +0.7% | -0.1% | +1.1% | +3.0% | +2.5% |
| `m_sort_block` | -1.0% | +0.5% | +206.4% | +13.8% | +7.9% |
| `m_sort_plain` | -0.3% | -4.6% | -8.8% | -6.4% | -2.7% |
| `m_yield_loop` | -1.1% | -0.0% | +4.6% | +2.5% | +1.4% |

- **W2 の Ruby のループ（`block-frames.rb`）は 2.5〜3 倍遅かった**: `index { }` +160%、`Array.new(n) { }` +149%、
  `sort { }` +206%。1 要素あたりの命令（`size`・`self[i]`・`yield` → `Proc#call` → `OP_CALL`・比較・`i += 1`）が、
  ネイティブの `call_block` 1 回より高くつく。計画の (a)「Ruby の前置き」も同じ形なので、同じだけ遅い。
  → **W2b でネイティブのループのフレーム**（`Vm::push_loop_frame`）に替えた。ループの状態をフレームのレジスタに置き、
  そのフレームは 1 命令だけ（`OP_DEBUG`）を持ち、その命令が「ネイティブのループを 1 歩進める」関数を呼ぶ
  （block をその上のフレームで呼ぶか、答えを返す）。block は普通のフレームなので待てる。`catch` も同じフレームにした
  （W2 の本家どおりのバイトコード版は +16%）。
- `Class#new` → `initialize`（+9%）と `Class.new { }`（+9%）: 答え用の `OP_RETURN R0` のフレームを 1 つ余計に積み・外す分。
  → W2b で **`Cci::KeepSelf`**（「そのフレームは返り値によらず R0 を答える。`break` のときだけ値を答える」）にし、
  余分なフレームを無くした。`break` が `ensure` を通って再開されても分かるよう、`BreakTag::BlockBreak` を足した
  （`Foo.new { break 9 }` が 9、`Class.new { begin; break 1; ensure; end }` が 1 のまま。テストで固定）。
- `Hash.new { }` の既定の proc（+29%）: `OP_GETIDX` が外れを send に回し、`Hash#[]` がもう一度引いていた。
  → `op_getidx` が自分で proc のフレームを積む（W2b）。
- `h[k]` の外れ（W2b で +10%）: W2 以来 `ivar_get` が 1 回増えていた。→ W2c で外れの処理を 1 か所（`hash_miss_at`）にし、
  `default` の再定義の確認をメソッドキャッシュで、名前を `Syms` から引くようにした（変更前は毎回 `intern` と
  キャッシュの無い探索をしていた）。結果は変更前より 21% 速い。
- `Object.new`（W2b で +3%）: `ruby_method` のキャッシュ引きが 1 回増えていた。→ W2c で `initialize` を 1 回だけ
  キャッシュで引き、それで `respond_to` も兼ねる（変更前はキャッシュの無い `respond_to` の後に `funcall`）。変更前より 9% 速い。
- フレームを積むときの `Vec`（引数の写し）をやめた（W2b）。

W2d（`sort!` の 1 歩で読むレジスタを減らす、ループの命令を `OP_NOP` にして operand の解読を省く）は、
`sort { }` を何も速くせず（W2c +7.7/+8.4%、W2d +8.5〜10%）、代わりに触っていない `bm_fib` を +0.7/+1.5% → +3〜4% に動かした。
W2e（ループの 1 歩を `#[inline(never)]` で `exec_frames` の外へ、`KeepSelf` の判定の並べ替え）も +3%。
`KeepSelf` の判定を消した試しのバイナリでも +3.5/+3.1% で、原因はそこではない。段階ごとに `bm_fib` を測ると
W1 −0.1%、W2 +3.8%、W2b +4.0%、W2c +0.4%、W2d +4.3% と、`bm_fib` の通る道（メソッド呼び出しと戻り）を変えていない
段階の間で上下する。`docs/design/optimizations.md` §4 の「置き場所のくじ」で、W2c の引きが良い。
**W2d を戻した**（`2bd48c2`。`src` は W2c と同じ木）。試しに `Vm` に足したフィールドを構造体の末尾に移したものも
1 巡では良くならなかった（2 巡目は PC が混んで読めない）。

### 最終（W2c = `2bd48c2` の `src`）

測ったバイナリは W2c（`f3c7d9c`、`src` は `2bd48c2` と同じ）。15:30 から別の担当のビルドがほぼ途切れずに走り
（`vmstat` の idle が平均 72%、0 まで落ちる）、17:00 まで待っても空かなかったので、静かだった巡だけを使う:
マイクロは 2 巡（13:45 の `2r` と 13:48 の `3`。どちらも巡の間の idle が 93% 以上）、`bench_ab.sh` は 2 巡
（15:08〜15:24 の `1` と `2r`、idle 93% 以上）。捨てた巡: マイクロの 1 巡目（idle 84% の標本あり）、
`bench_ab.sh` の 3 巡目（idle 0% まで。`vm_optimization_bench` +26.5% はこの巡）。
結果の TSV は `bench/results/wait-anywhere/`、マイクロの元は `bench/micro-wait/`。

マイクロ（ms、全巡の最速。変更前 → W2c）:

| ベンチ | 変更前 | W2c | 差 | 巡ごと |
|---|---:|---:|---:|---|
| `m_array_new_block` | 143.397 | 140.954 | -1.7% | -1.7 -1.1 |
| `m_catch` | 145.956 | 145.189 | -0.5% | -0.6 -0.5 |
| `m_class_exec` | 82.428 | 76.216 | -7.5% | -7.1 -7.5 |
| `m_class_new_block` | 54.815 | 53.630 | -2.2% | -5.6 -2.2 |
| `m_hash_default_proc` | 73.695 | 54.797 | -25.6% | -25.6 -25.7 |
| `m_hash_hit` | 141.146 | 135.209 | -4.2% | -4.3 -4.2 |
| `m_hash_miss` | 174.862 | 138.392 | -20.9% | -21.1 -20.9 |
| `m_index_arg` | 297.531 | 297.234 | -0.1% | +2.1 -0.1 |
| `m_index_block` | 261.830 | 227.304 | -13.2% | -11.4 -13.2 |
| `m_instance_eval` | 88.460 | 81.664 | -7.7% | -6.4 -7.7 |
| `m_instance_exec` | 92.701 | 84.685 | -8.6% | -9.4 -8.6 |
| `m_method_call` | 83.976 | 80.906 | -3.7% | -3.7 -3.7 |
| `m_method_loop` | 81.839 | 84.226 | +2.9% | +3.5 +0.4 |
| `m_new_plain` | 149.426 | 136.204 | -8.8% | -10.3 -7.2 |
| `m_new_ruby_init` | 208.861 | 198.521 | -5.0% | -4.8 -7.2 |
| `m_plain_loop` | 92.957 | 93.608 | +0.7% | +1.7 -1.7 |
| `m_public_send` | 70.033 | 65.606 | -6.3% | -5.8 -6.3 |
| `m_send` | 122.514 | 122.789 | +0.2% | +0.2 +1.3 |
| `m_sort_block` | 272.629 | 294.657 | +8.1% | +8.7 +8.1 |
| `m_sort_plain` | 242.366 | 250.387 | +3.3% | +3.4 +3.3 |
| `m_yield_loop` | 161.558 | 163.849 | +1.4% | +2.4 +1.4 |

`bench_ab.sh`（ms、同じ読み方）:

| ベンチ | 変更前 | W2c | 差 | 巡ごと |
|---|---:|---:|---:|---|
| app_json_hash | 1212.711 | 1213.254 | +0.0% | -0.0 +0.0 |
| app_robot | 1133.318 | 1088.464 | -4.0% | -4.8 -3.5 |
| app_tak | 1112.499 | 1131.858 | +1.7% | +0.7 +1.7 |
| bm_ao_render | 5403.696 | 5287.048 | -2.2% | -2.2 -2.2 |
| bm_fib | 5085.598 | 5161.792 | +1.5% | +1.5 +1.4 |
| bm_mandel_term | 18.085 | 18.139 | +0.3% | -0.2 +0.3 |
| bm_so_lists | 933.161 | 987.701 | +5.8% | +5.9 +5.8 |
| bm_so_mandelbrot | 1274.874 | 1281.721 | +0.5% | +0.5 +0.6 |
| call_args | 852.549 | 855.283 | +0.3% | -0.6 +0.9 |
| call_block_yield | 982.258 | 989.729 | +0.8% | +1.0 +0.6 |
| call_fiber | 879.955 | 892.631 | +1.4% | +1.4 +1.9 |
| call_kwargs | 806.828 | 811.449 | +0.6% | +0.6 +0.1 |
| ds_array | 821.587 | 838.725 | +2.1% | +2.2 +1.6 |
| ds_hash | 218.884 | 213.141 | -2.6% | -2.6 -1.4 |
| ds_string | 359.940 | 375.795 | +4.4% | +4.4 +3.4 |
| gc_churn | 364.982 | 361.670 | -0.9% | -0.9 -0.8 |
| loop_if_branch | 835.354 | 840.954 | +0.7% | +0.5 +2.3 |
| loop_times | 835.046 | 843.119 | +1.0% | +1.0 +0.3 |
| loop_while_add | 811.821 | 823.618 | +1.5% | +1.6 -1.4 |
| mem_retained | 722.646 | 682.305 | -5.6% | -5.6 -5.7 |
| mem_short_lived | 980.499 | 871.909 | -11.1% | -11.1 -11.4 |
| vm_optimization_bench | 10698.981 | 11037.000 | +3.2% | +3.2 +2.3 |
| vmo_arith | 3608.763 | 3648.287 | +1.1% | +1.7 +1.1 |
| vmo_calls | 2817.807 | 2870.355 | +1.9% | +1.7 +2.2 |
| vmo_dispatch | 2096.661 | 2158.294 | +2.9% | +2.9 +4.1 |
| vmo_index | 499.364 | 495.782 | -0.7% | -1.1 -0.7 |
| vmo_objects | 590.560 | 604.646 | +2.4% | +4.2 -1.5 |

全 27 本の合計は巡ごとに +0.9% と −0.7%（A/A の 3 巡は +0.3%、−0.1%、−0.0%）。

**線（マイクロ 2.7%、`bench_ab.sh` 6.6%、全巡で正）を超えたもの:**

- `m_sort_block`（`sort { |x, y| x <=> y }` を 50 要素で）**+8.1%**（+8.7、+8.1）。W2d・W2e の手直しでも縮まなかった
  （+8.5〜10%）。1 回の `sort` で 240 回比べるので、比べ 1 回あたり約 114 ns → 123 ns。ネイティブのマージソートは Rust の局所変数でループを回し、比べるたびに
  `call_block` で入れ子のループを起こす。ループのフレームは、比べるたびに命令ループを 1 周（`OP_DEBUG`。`--stats` の命令数が
  12,162,140 → 14,562,140 と、比べた回数 2,400,000 だけ増える）してレジスタから
  状態を読み書きし、block のフレームを積む。block の中身が `x <=> y` のように軽いと、この差が見える。
  Ruby で書いた版（W2、計画の (a) に当たる）は +206%。**直し方を変えても線の内側に入れられなかったので、ここで止めて報告する。**
  選べるもの: (1) このまま（`sort { }` の中で待てる、軽い block で +8%）、(2) `sort! { }` だけネイティブの入れ子に戻す
  （`array.rs` の `sort!` の 1 行。`sort { }` は名指しの境界に戻る）、(3) 段階 3 の「再開できるネイティブ」を待つ。
- `m_method_loop` +2.9%（+3.5、+0.4）と `m_sort_plain` +3.3%（+3.4、+3.3）。どちらも**この変更が触った道を通らない**
  （`def f(x) = x + 1` の呼び出しと、block の無い `sort`）。命令数は前後で同じ。段階ごとに `bm_fib` を 1 巡ずつ測ると触っていない段階の
  間で −0.1〜+4.3% 動いたのと同じく、`exec_frames` の置き場所の揺れと読む（下の `bench_ab.sh` の so_lists も同じ）。

**線の内側だが、全巡で同じ向きに動いたもの:** `bm_so_lists` +5.8%（+5.9、+5.8。W1 でも +5.6、+3.9）、`ds_string` +4.4%、
`vm_optimization_bench` +3.2%、`vmo_dispatch` +2.9%、`ds_array` +2.1%、`vmo_calls` +1.9%、`bm_fib` +1.5%。
`bm_so_lists` と `ds_string` と `vmo_dispatch` と `bm_fib` は `--stats` の命令数が前後で 1 命令も違わず、
1 命令あたりの時間だけが伸びている（`bm_so_lists` 16.6 → 17.5 ns）。通る道の変わっていない「置き場所」の差で、
`bench.md` の `codegen-units = 1` の節が言う「くじ」がこの枝で悪く出たもの。直す手（`Vm` に足したフィールドを構造体の
末尾へ移す）は 1 巡では効かず、PC が混んでいて確かめきれなかった。

**速くなったもの:** `Hash.new { }[k]` −25.6%、`h[k]` の外れ −20.9%、`index { }` −13.2%、`Object.new` −8.8%、
`instance_exec` −8.6%、`instance_eval` −7.7%、`class_exec` −7.5%、`public_send` −6.3%、`Class#new` → `initialize` −5.0%、
`mem_short_lived` −11.1%、`mem_retained` −5.6%、`app_robot` −4.0%。入れ子のループを起こさなくなった分と、
`Class#new` と `Hash#[]` の外れがメソッドキャッシュを使うようになった分。

マイクロの `N`（1 本 50〜300 ms になるよう、変更前の命令数から決めた）: plain_loop 2,000,000、method_loop・yield_loop・send・
new_ruby_init・new_plain・index_arg 1,000,000、hash_hit・hash_miss 2,000,000、instance_exec・instance_eval・class_exec・
method_call・public_send・hash_default_proc 500,000、sort_plain・catch 300,000、class_new_block 200,000、index_block 100,000、
array_new_block 50,000、sort_block 10,000。

## 著者の判断: `sort { }` は入れ子のままにする（2026-09-26）

上の「線を超えたもの」の 3 択のうち、著者は **(2) `sort! { }` だけネイティブの入れ子に戻す** を選んだ（2026-09-26）。
`sort { }` は名指しの境界 `can't wait inside Array#sort!'s call to a block` に戻る（本家も境界）。

- `array.rs` の `sort!` から `in_frame` の枝を外し、`sort_step` / `sort_in_frame` / `LOOP_SORT` を消した。
  ループのフレームで一番多く状態を持つのが `Array.new(n) { }` の 3 個になったので、`LOOP_RESULT` を 16 → 7 にした。
- probe の期待に `c_sort` を名指しの境界として戻した（`tests/wait_anywhere.rs`）。

### 測り直し（`m_sort_block`・`m_sort_plain`）

変更前（`3170b63`）と交互に、各巡 5 回、`--core 2`、巡ごとに順を入れ替え、`vmstat -t 10` を記録（全巡 idle 95% 以上）。
**入れ子に戻しても `m_sort_block` は線（マイクロ 2.7%）の内側に戻らなかった。** 同じ道を通る木を 5 通り建てて測った:

| 建てたもの | `m_sort_block` | `m_sort_plain` | `bm_so_lists` |
|---|---|---|---|
| `1eee048`（入れ子に戻しただけ） | +6.6%（+8.2 +7.2 +4.6） | −4.9% | — |
| fin2（`unwind_return` の末尾を W2c 前の形に近づけた） | +3.7%（+3.7 +3.7） | −6.5% | +5.5% |
| fin3（fin2 + ループの 1 歩を `#[inline(never)]` で外へ） | +1.0%（+2.1 +0.3） | **+10.7%**（+10.6 +10.7） | +0.2% |
| fin4（fin3 + `sort_values` を変更前と同じ字面に戻す） | +6.5%（+6.6 +5.4） | +2.6% | +1.1% |
| fin5（fin4 + block なしの Integer だけの配列は Rust の `sort_unstable`） | **+5.4%**（+3.0 +5.4 +5.4） | −36.9% | +0.4% |

`m_sort_block` の命令数は変更前と同じ（12,162,140）。block を呼ぶ道（`call_block` → 入れ子の `run_loop` →
`OP_ENTER`・`<=>`・`OP_RETURN` → `unwind_return`）で変更前と違うのは、`run_loop_ctx` の `direct_send` の出し入れ
（W1 から。W1 の `m_sort_block` は +0.1、−2.5%）と、`unwind_return` の末尾の `Cci::KeepSelf` の比較 1 つ。
同じ道のまま 5 通りの建て方で +1.0〜+6.6% に散り、`m_sort_plain` と `bm_so_lists`（触っていない道）も −6.5〜+10.7%、
+0.2〜+5.5% に散るので、`exec_frames` まわりの置き場所の揺れが大半と読む。ただし 5 通りのどれでも正の側なので、
比較 1 つ分の実費も混じっていると思われる（分けられていない）。

残したのは fin5（`sort` の block なし・Integer だけの配列は、比べる関数が `p.cmp(&q)` を答えるだけで何も呼ばず、
等しい Integer は同じ値なので、どの並べ方でも結果は同じ）。`m_sort_plain` は −37%。ここは本来の範囲の外の手直しだが、
`m_sort_plain` を置き場所のくじから外すために入れた。

**`m_sort_block` は +5.4% のまま（線 2.7%）。これ以上は直し方が見つからず、ここで止めて報告する。**

### `tools/bench_ab.sh` の順の偏り

`bench_ab.sh` は各回で必ず A を先に走らせていた。A/A の 1 巡目は B 側に平均 +0.8% ほど寄っていた。回ごとに先に走る方を
入れ替えるようにした（`for i in $(seq "$RUNS")` で奇数回は A が先、偶数回は B が先）。上の測り直しはこの版で取った。

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
- `tools/bench_ab.sh` は各回で必ず A を先に走らせていた。A/A の 1 巡目は B 側に平均 +0.8% ほど寄っていた。
  回ごとに順を入れ替えるよう直した（上の「`tools/bench_ab.sh` の順の偏り」）。
- 同じ PC で別の担当の C のビルドとテストが 13:24〜17:00 過ぎまで断続的に走り、`bench_ab.sh` の巡の中で idle が 0% まで
  落ちた。巡の前に idle を見るだけでは足りず、巡の間の `vmstat -t` を残して後から捨てる形にした。計測の手順の話。
- `m_sort_block` の扱いは著者が (2) に決めた（上の「著者の判断」）。入れ子に戻しても +5.4% が残っている。

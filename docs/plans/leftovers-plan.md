# 小さな残り（実装指示書）

作成 2026-09-16。各段階が「判断待ち」「範囲外」として残した小さな項目を 1 か所に集めたもの。どれも半日以内。
著者の判断: `from-mrubyedge-plan.md` と `perf3-plan.md` の間か、implementer の手が空いたときに。1 つずつ別コミット、worklog は 1 本（`YYYY-MM-DD-leftovers.md`）。

## 状況

| # | 内容 | 出どころ | 状態 |
|---|---|---|---|
| 1 | `foo(**{})`（空のキーワード Hash）を `native_args` が落とす。本家は届ける | 6c | **済み**（2026-09-16、`ce3a134`） |
| 2 | マクロの生成する `register` に残る `expect` 1 つ → `VmResult` を返す形に | 6a | **済み**（2026-09-16、`758e1a2`） |
| 3 | `Player.allocate` で作った素のオブジェクトにメソッドを呼んだときの文言（`wrong argument type Player (expected Player)`）を読めるものに | 6a | **済み**（2026-09-16、`8d7bd9d`） |
| 4 | `#[ruby_methods]` でブロックを取るメソッド（`define_fn` の `Block` を通す） | 6a | **済み**（2026-09-16、`a9461d8`） |
| 5 | `Kernel#printf` / `#putc` が無く、本家のベンチ `bm_ao_render` と `bm_mandel_term` が動かない | 段階 1 から | **済み**（2026-09-16、`519eb17`、基準の取り直しは `a17c154`）。出どころは `mruby-print` ではなく `mruby-io`。ベンチは 27 本になり、2 本は 2.25x と 2.02x |
| 6 | `items()` が配列を複製する箇所の残り（中身を読むだけのもの。ベンチには出ないが実コードで効く） | 2d | **済み**（2026-09-16、`e27d9a4`・`87ad19b`・`e877d17`）。micro で −52%／−59%／−17%。全体を答えるものは借用にしても意味が無いので採らなかった |
| 7 | rubevy の `Rubevy.ask` の `Arg` に Hash/Array を運べない（`Arg::Value`。今は数値と文字列と Entity だけ） | ECS の橋 A | **済み**（2026-09-16、rubevy `0142f93`）。`Request` の drop で解放（`Vm` が無いので次の `tick_scripts` 先頭で `gc_unregister`）。記録は rubevy `docs/worklog/2026-09-16-arg-value.md` |
| 8 | SabiRuby の公開 API で足りなかったもの: Hash のキー列挙、`Task::Queue` の長さと非ブロッキング pop（rubevy が `funcall` で代用している） | ECS の橋 B | **済み**（2026-09-16、`6af8276`）。rubevy 側の置き換えも済み（rubevy `83cd763`） |
| 9 | coverage が見つけた「本家にあって本当に無い」もの: `Hash#default_proc=`、`Numeric#fdiv`、`Module#const_added`/`#method_undefined`、`BasicObject#singleton_method_added`/`_removed`/`_undefined` | coverage | **済み**（2026-09-16、`ef4611f`）。`coverage.md` の「本家だけ」22 → 15 |
| 10 | 可視性の食い違い 50 件（本家が private、SabiRuby が public。`Module#private`/`module_function`/`included`/`method_added` の類）と、トップレベルの `def` が private にならない件 | coverage | **済み**（2026-09-17、`835ac67`・`43bd12f`）。著者の判断は「本家のとおりに直す」。coverage の「可視性が違う」50 → 0 |

## 各項目

1. **`foo(**{})`**: `native_args` がキーワード Hash を「空なら無かったことに」している箇所を本家（`mrb_get_args` の `:` と `OP_ENTER` の kw の扱い）と突き合わせ、
   空でも届ける。`tests/custom/` に本家の出力で期待値を置く。`send` と `method_missing` の再ディスパッチも同じ経路なので一緒に直る。
2. **`register` の `expect`**: `singleton_class(...)` が失敗しうるのは到達不能のはずだが、生成コードに `expect` を残さない。`T::register(&mut Vm) -> VmResult<ObjId>` にする
   （破壊的変更だが利用者はまだ無い）。`macros/tests/expand.rs` の固定を更新。
3. **`allocate` の文言**: `This<DataRef>` の変換失敗を「`Player` のインスタンスが Rust の実体を持っていない（`allocate` で作られた）」と分かる文言に。
4. **ブロックを取るメソッド**: `#[ruby_methods]` の `fn f(&self, vm: &mut Vm, blk: Block)` を通す（`define_fn` に形はある。5 行程度）。テストを `player.rs` に 1 本。
5. **`printf` / `putc`**: 本家の `mruby-print`/`mruby-sprintf` を見て、`Kernel#printf`（`sprintf` の上に）と `#putc` を足す。`bm_ao_render` と `bm_mandel_term` が動くようになるので、
   `bench/categories.tsv` に分類を足し、基準を取り直す（`docs/verification/bench.md` に「23・24 本目」として）。
6. **`items()` の残り**: `grep -n "items(vm" src/builtins/` で一覧を作り、中身を読むだけのものから `Heap::array` の借用に。ブロックを呼ぶものは添字で。
   micro（`ab_micro.sh` の流儀）で 1 つずつ前後を取る。
7. **`Arg::Value`**: `Rubevy.ask` の引数に Hash/Array を許す。`Value` を `gc_register` して運び、答えた後に解除する（`Request` が drop されたときに解除する形にすると漏れない）。
   `Request::value(i)` と、`FromRuby` で取り出す補助。
8. **公開 API の穴埋め**: `Vm::hash_keys(h) -> Vec<Value>`、`Vm::task_queue_len(q)`、`Vm::task_queue_try_pop(q) -> Option<Value>`。rubevy の `funcall` 代用を置き換える。

## 守ること

`host-bridge-plan.md` と同じ。5 と 6 は性能に触るので交互 A/B（5 はベンチの本数が変わるので基準の取り直し）。
9. **coverage の穴**: `docs/verification/coverage.md` の「本家だけ」22 件のうち効くもの。`Hash#default_proc=` は本家 `mrb_hash_default_proc_set`（`hash.c`）、
   `Numeric#fdiv` は `mruby-numeric-ext`、フックは `mrb_method_added` 系の呼び出し箇所（`class.c`）と突き合わせる。各 1 本 `tests/custom/`。終わったら `tools/coverage.sh` で生成し直す。
10. **可視性**: 50 件の一覧は `coverage.md` の「両方にあるが可視性が違う」。直すなら `define_methods` に可視性を渡す形（本家の `MRB_METHOD_PRIVATE_FL`）と、
    `OP_DEF` がトップレベル（`self` が main）で private にする本家の規則（`vm.c` の `OP_DEF` → `mrb_define_method_raw` の可視性）。`respond_to?` と `send` の差にだけ効くので、
    mrbtest には出ていない。著者が「意図した差分」と決めるなら `docs/design/gems.md` の Deviations kept に 1 項。



## 実装で分かったこと（2026-09-16、項目 1・2・3・4・8・9）

作業の記録は [`../worklog/2026-09-16-leftovers.md`](../worklog/2026-09-16-leftovers.md)。
確認: `cargo test --workspace` 全通過、`tools/check_no_std.sh` 通過、
`tools/mrbtest.sh` の 3 ビルドとも `tests/mrbtest/baseline*.txt` と一致（2507 中 2344 通過、変化なし）、
VM の crate の `unsafe` 0、`cargo doc` の rustdoc 警告 0。

1. **空のキーワード Hash は「畳み込まない」であって「無い」ではない。** 本家の `mrb_get_args` も
   `vm_op_enter` も、キーワード Hash を位置引数に畳み込むのは中身があるときだけだが、**空のときは `ci->kw` を立てたまま**残す。
   `OP_ENTER` の速い経路が `argc + ci->kw != m1` と数えるので、本家では `def one(x); end; one(**{})` の `x` が `{}` になる
   （CRuby は ArgumentError なので mruby の方言）。SabiRuby はバイトコードからの直接の呼び出しだけこの規則を写していて、
   フレームを書き換えて配り直す `send` と `method_missing` で落としていた。
   * **`send` と `method_missing` はフレームの作り方が違う。** `send_method` は呼ばれた形を保つ（`n == 15` は 15 のまま
     `mrb_ary_subseq`）が、`prepare_missing` は `mrb_args_pack_positional` を通るので**常に `n = 15`**。
     `2026-09-15-stage6c-method-missing.md` の「詰めるのとずらすのは `OP_ENTER` から見れば同じ」は、
     キーワード Hash があるときだけ成り立たない（速い経路は `argc < 15` でしか通らない）。
     `Enumerator#__enumerator_block_call` の `@obj.__send__ @meth, *@args, **@kwd, &block` が、ほどくと落ちる実例。
   * **残った差**: `public_send` はネイティブから `vm.funcall` へ落ちるので空のキーワード Hash を運べない。
     本家の `public_send` は `send_method(mrb, self, TRUE)` で `send` と同じ経路なので、
     `op_send_redirect` に可視性の判定を足せば揃う（今回の範囲外）。
2. **`register` の `expect`** は消えた（`T::register(&mut Vm) -> VmResult<ObjId>`）。rubevy はまだ `#[ruby_methods]` を
   使っていない（クラス登録は手書き）ので、**rubevy に直す呼び出しは無い**。`rubevy/src/lib.rs:1215` の
   `vm.singleton_class(...).expect("Rubevy singleton")` は rubevy 自身が書いたもので、これとは別。
3. **本家に同じ文言がある。** `mrb_data_check_type`（`src/etc.c`）は `DATA_TYPE` が NULL のとき
   `uninitialized %t (expected %s)` を投げ、`%t` はオブジェクト自身のクラス名。`Time.allocate.to_i` が
   `uninitialized Time (expected Time)`、部分クラスなら `uninitialized MyTime (expected Time)`。それに寄せた。
   `convert.rs` の `DataRef::from_ruby` は期待するクラスを持たないので触っていない（本家も引数側は
   `wrong argument type Object (expected Data)` で、SabiRuby の今の文言と一致している）。
4. **`define_fn` には最初から形があった**（`MVmBlock`、`MVmThisBlock`）。マクロが読んでいなかっただけで 5 行。
   `Block` 無しの形が `define_fn` に無いので、`Block` を取るメソッドは `&mut Vm` も取る（ブロックを呼ぶのに要る）。
   ブロックは arity に数えない。`&mut Vm` を取るメソッドは受け手を店から出したまま走るので、
   **ブロックの中から同じオブジェクトに触ると弾かれる**（`Player is already in use by a call on the same object`）。
5. **rubevy が `funcall` で代用していたのは 3 か所**（項目 8）: `reflect.rs:136`（`:keys`）、
   `lib.rs:782`（`:size`）、`lib.rs:789`（`:__pop_try(true)`）。後ろの 2 つは `ScriptWorld::make_room` の中で毎フレーム回りうる。
   `hash_keys` は `hash_entries` と同じ形（挿入順、Hash でなければ `None`、Ruby を走らせない）に揃えた。
   `task_queue_try_pop` は空なら `None`（ホストはタスクではないので park するものが無く、
   `Task::Error` を上げるのも違う）。閉じたキューも `None`（区別が要るなら `closed?`）。
6. **`Numeric#fdiv` は `mruby-numeric-ext` ではなくコア**（`src/numeric.c` の `numeric_rom_entries`）。
   計画書の出どころが違っていた。`const_added` も読み違えた: `mrb_vm_define_class` は直接 `mrb_const_set` を
   呼ばないので `class Foo; end` では発火しないと思ったが、`setup_class` の中身が `mrb_const_set` そのもので、
   **クラス定義でも発火する**。本家で期待値を作って初めて分かった。
7. **`singleton_method_added` の既定が `Object` にあったせいで、判定が一度も当たっていなかった。**
   `Vm::method_added` の「既定の no-op なら呼ばない」は `owner == Module || owner == BasicObject` を見る。
   本家は `bob_rom_entries`、つまり `BasicObject`。`BasicObject` へ移して初めてその判定が効く
   （それまでは特異メソッドを定義するたびに `funcall` が 1 本走っていた）。
   `ext_metaprog.rs` の `remove_method` は既定を確かめずに無条件で `funcall` していたので、
   `singleton_method_removed` の既定が無いまま特異メソッドを `remove_method` すると NoMethodError になっていたはず。
8. **見つけたが直していない差**（どれも今回の項目の外）:
   * `7.fdiv(0)`: 本家は ZeroDivisionError（`int_fdiv` の `if (y == 0) mrb_int_zerodiv`）、SabiRuby は `Infinity`。
   * `1.0.fdiv("2")`: 本家 `String cannot be converted to Float`、SabiRuby `String can't be coerced into Float`。
   * `public_send(**{})`（上の 1 を参照）。

### 7. `Arg::Value`（rubevy 側）

* `Drop` に `Vm` が来ないので、`Root::drop` は `ObjId` をキューに積み、`ScriptWorld::release_dropped_values`（`tick_scripts` の先頭）が
  `gc_unregister` する。値が request より最大 1 フレーム長生きするが、早く死ぬよりよい。`Request` は `Clone` のまま（`Arc<Root>`）。
* `gc_register` を外すと `access to freed object` になることを negative control で確認。解放は GC 後の生存数で実測（1043 → 633）。
* 振る舞いの変更 2 点: `nil`/`true`/`false` と `to_s` を持つ独自オブジェクトは `Arg::Text("")` に潰れていたのが `Arg::Value` で届く。

### 見つけたが直していない差（項目 1・9 の副産物、範囲外）

* `public_send(:one, **{})` — 本家 `{}`、SabiRuby `ArgumentError`。`public_send` が `vm.funcall` に落ちるため。`op_send_redirect` に可視性検査付きで通せば閉じる。
* `7.fdiv(0)` — 本家 `ZeroDivisionError`、SabiRuby `Infinity`。`1.0.fdiv("2")` の文言も違う。
* `docs/verification/mrbtest-bytes.md` が `notes.tsv` に対して古い（数値は不変）。



## 実装で分かったこと（2026-09-16、項目 5・6）

作業の記録は [`../worklog/2026-09-16-leftovers-perf.md`](../worklog/2026-09-16-leftovers-perf.md)。
確認: `cargo test --workspace` 202 件全通過、`tools/check_no_std.sh` 通過、
`tools/mrbtest.sh` の 3 ビルドとも `tests/mrbtest/baseline*.txt` と一致
（既定 2507 中 2344、バイト 2452 中 2267、regexp 無し 2011 中 1979。いずれも変化なし）、
VM の crate の `unsafe` 0、`cargo doc` の rustdoc 警告 0。

### 5. `printf` / `putc`

1. **出どころは `mruby-print` ではなく `mruby-io`。** 4.1.0-rc に `mruby-print` という gem は無い。
   `printf` も `putc` も `mrbgems/mruby-io/mrblib/kernel.rb` の `module_function`
   （`$stdout.printf(...)` と `$stdout.putc(c); nil`）で、実体は `IO#printf`（`mrblib/io.rb:280` の
   `write sprintf(*args)`）と `io_putc`（`src/io.c:1112`）。`mruby-sprintf` が持っているのは
   `sprintf`/`format` だけ。SabiRuby には IO が無いので、`print` と同じ出口に書くネイティブにした。
2. **返り値は doc のとおり両方 nil。** `IO#printf` は `write` の答えを返しそうに見えるが、
   本家で実際に走らせると `printf` は nil。`putc` も `Kernel` 側は `; nil` で潰してあるので nil
   （`IO#putc` は引数を返す）。**引数の誤りは全部 `sprintf` の例外**で、`printf` 自身は引数を数えない。
3. **`putc` の Integer は `c & 0xff` の 1 バイト**。UTF-8 のビルドでもコードポイントではない
   （`putc(0x2603)` が `0x03`）。Integer 以外は `mrb_obj_as_string` を通して**最初の 1 文字**。
4. **「1 文字」はビルドの都合で、文字列の都合ではない。** `io_putc` は `MRB_UTF8_STRING` のとき
   `mrb_utf8len` を使い、その文字列が binary（`String#b`）かどうかは見ない。
   `-rc-utf8` の `putc("\u2192".b)` が 3 バイト書くのを実測して確かめた。だから `char_mode` ではなく
   `string::UTF8` を渡している。
5. **残した差**: 本家の `printf` は Ruby から `sprintf` を呼ぶので `Kernel#sprintf` の再定義が効くが、
   こちらは書式化器（`ext_sprintf::sprintf_bytes`）を直接呼ぶので効かない。`bm_mandel_term` の内側で
   `putc` が 1 ピクセル 1 回走るので、`funcall` を挟まない形を選んだ。
6. **ベンチ側で足すものは無かった。** `bench/bm_ao_render.mrb`・`bm_mandel_term.mrb` も
   `bench/categories.tsv` の行も最初からあり、`fail:` の行が結果に残っていただけだった。
   確かめるのは出力で、本家と 1 バイトずつ一致する（3900 バイトと 12301 バイト）。
   `bench/src/bm_app_lc_fizzbuzz.rb` だけは本家の `mrbc` が `syntax error, unexpected ']'` で受け付けず、
   `.mrb` ができないのでベンチの本数に入らない（元からそう）。
7. **見つけたが直していない差**: private なメソッドに対する `respond_to?` が本家 `true`、SabiRuby `false`
   （`Object.new.respond_to?(:puts)` でも同じなので `printf` の話ではない）。項目 10 の範囲。

### 6. `items()` の残り

1. **「複製している」だけでは直す理由にならない。** `vm.ary_new` が `Vec<Value>` を取り、その中で
   もう 1 回 `Vec<Slot>` にするので、**答えが配列全体なら複製は 2 回が下限**。借用に書き換えても回数は変わらない
   （`reverse`・`rotate`・`compact`・`+`・`*`・`uniq`。手を付けなかった）。効くのは
   (a) 答えが 1 要素か数要素なのに全体を複製していたもの、(b) `Vec<Slot>` のまま済むのに
   `Value` を経由していたもの（`dup`・`initialize_copy`・`replace`）の 2 通りだけだった。
2. **引数の変換は借用の前に。** `expect_int` は `to_int` を呼びうる、つまり Ruby を走らせて配列を動かしうる。
   `Array#[]` の `index_args` だけは Integer と Integer の Range しか読まない（Bignum は即例外）ので
   Ruby を走らせず、その後に借りてよい。
3. **要素ごとに Ruby を呼ぶ走査は速くならない。** `Array#-`・`&`・集合演算・`sort_by` などは `items` を
   ループの**外で 1 回**呼ぶのですでに O(n) で、1 要素あたりのコストはディスパッチが支配する。
   実測でも雑音の底（−1% 前後）。しかも添字で読み直すと snapshot でなくなるので**意味が変わる**。
4. **ただし `assoc`/`rassoc`/`__ary_index` は別**。本家の `ary_assoc` は `RARRAY_LEN`/`RARRAY_PTR` を
   毎回読み直し、コメントで「`mrb_equal` が消すかもしれない」と言っている。SabiRuby の
   `index`/`include?`/`member?`/`__count` はすでにその形で、この 3 つだけ取り残されていた。
   速さではなく形を合わせるために直した。
5. **micro を測る道具を足した**（`bench/micro`、`tools/ab_micro.sh`）。公開ベンチ 27 本は増やさない。
   **同じラウンドの空ループの動きを雑音の底として引く**のが要で、同じ変更が空ループ −0.0% の回では −1.5%、
   −2.0% の回では −2.9% に出た。
6. **`tools/bench.sh` は `SABIRUBY_BIN` が無いと作業ツリーをビルドして測る。** 未コミットの項目 6 を
   置いたまま基準を取り直しかけて、途中で気づいて捨てた。以後は測るコミットのバイナリを先に作って名指ししている。


## 実装で分かったこと（2026-09-17、項目 10）

作業の記録は [`../worklog/2026-09-17-visibility.md`](../worklog/2026-09-17-visibility.md)。
確認: `cargo test --workspace` 全通過（`tests/custom` は 17 → 21 本）、`tools/check_no_std.sh` 通過、
`tools/mrbtest.sh` の 3 ビルドとも `tests/mrbtest/baseline*.txt` と一致
（既定 2507 中 2344、バイト 2452 中 2267、regexp 無し 2011 中 1979。いずれも変化なし）、
VM の crate の `unsafe` 0、`cargo doc` の rustdoc 警告 0。
`coverage.md` の「両方にあるが可視性が違う」は **50 → 0**、
「モジュール関数の片割れ」は 3 → 1、「本家だけ」は 346 → 344。

1. **「`initialize` はいつも private」は組み込みの表には届かない。** 本家の規則は
   `mrb_define_method_raw`（`src/class.c:1071`）にあるが、組み込みのメソッド表は
   `MRB_MT_INIT_ROM`（`mrb_mt_init_rom`）が const な項目配列をそのまま層として繋ぐので
   **その関数を通らない**。だから各 ROM テーブルは自分で `| MRB_MT_PRIVATE` と書いており、
   書き忘れた `mruby-struct` と `mruby-random` の `initialize` は本家で public のまま。
   規則を `Vm::def_method_raw` に移したら coverage が 50 → 2 になって、残った 2 件がそれだった。
   規則は `Vm::def_method`（`def`・`alias_method`・`define_method` が通る道）に残し、
   組み込み側は本家の表と同じく private にする項目を挙げる形にした。
2. **`Module#method_removed` は本家でも public。** core の `mod_rom_entries` は
   `MRB_MT_PRIVATE` で定義しているが、`mruby-metaprog` がフラグ無しで定義し直す
   （`metaprog.c:726`）。後の層が勝つ。coverage の 50 件に入っていなかった理由。
3. **トップレベルの `def` の規則は「self が main か」ではない。** 計画書の想像とは違い、
   `OP_DEF` は `MRB_METHOD_VDEFAULT_FL` を立てるだけで、**文脈の土台のフレームだけが
   private で始まる**（`src/vm.c:136`、`c->ci->vis = 1`）。`cipush` が作るフレームは全部 public
   （`src/vm.c:868`）。だから `eval("def x; end")` の `x` は **public**（`mrb_top_run` が
   入れ子では `cipush` する）。SabiRuby では `run_irep`/`start` が土台に当たるので
   「`ci` が空なら private」の 1 行。ブロックは env が `ci.vis` を写すのでついてくる。
4. **`Module#define_method` は場の可視性を見ない。** `mrb_mod_define_method_m` は
   `MRB_METHOD_PUBLIC_FL` を書いて渡す（`src/class.c:4197`）ので、`private` の下でも
   `module_function` の下でも public。SabiRuby は `current_def_vis` を見ていて、
   トップレベルが private になった途端に `tests/custom/method_cache.rb` が落ちて分かった。
   トップレベルの `define_method` だけが private なのは、本家が main の特異クラスに
   `top_define_method` を別に置いているから。**SabiRuby には main の `define_method` が無い**
   （項目 9 の側の話なので足していない）。
5. **本家の `respond_to?` は可視性を見ない。** `obj_respond_to`（`src/kernel.c:624`）は
   `include_all` を `respond_to_missing?` に渡すためだけに受け取り、メソッドが見つかったときは
   使わない。だから本家では `Object.new.respond_to?(:puts)` も `respond_to?(:initialize)` も true
   （doc コメントは CRuby 風に書いてあるが、コードは見ていない）。SabiRuby は弾いていたので、
   本家に合わせて弾くのをやめた。`2026-09-16-leftovers-perf.md` が「見つけたが直していない差」に
   挙げていた件がこれで閉じた。
6. **`send`・`public_send`・`Object#method`・`methods`/`private_methods`・`instance_methods` 系は
   もともと本家と同じ**で、変更は要らなかった。NoMethodError の文言も一致する
   （`private method 'top_m' called for Object`、`protected method 'prot' called for Prot`）。
7. **残した差**: `` Kernel.` ``（モジュール関数の片割れ）を足していない。足すと
   本家の `test/t/syntax.rb` の `External command execution.` が落ちる
   （`self` が Kernel のとき特異メソッドが先に当たり、SabiRuby の本体は
   `NotImplementedError`）。`docs/design/gems.md` の Deviations kept に 1 項。

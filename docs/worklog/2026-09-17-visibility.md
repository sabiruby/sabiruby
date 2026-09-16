# 可視性を本家に合わせる（`leftovers-plan.md` 項目 10）

2026-09-17。`docs/plans/leftovers-plan.md` の項目 10。著者の判断は「本家のとおりに直す」。
`docs/verification/coverage.md` の「両方にあるが可視性が違う」50 件と、トップレベルの `def` が
private にならない件。ブランチ `visibility`、main は `1c747bf`。参照は `/home/kishima/book/ref/mruby`。

## 1. 本家はどこで private を決めているのか

まず本家の仕組みを読んだ。`MRB_METHOD_PRIVATE_FL`（`include/mruby/proc.h:197`）を立てる経路は 3 つある。

* **ROM テーブルの項目**。`MRB_MT_ENTRY(fn, sym, MRB_ARGS_*() | MRB_MT_PRIVATE)`
  （`include/mruby/class.h:132`）。`src/class.c` の `bob_rom_entries`・`cls_rom_entries`・
  `mod_rom_entries`、`src/kernel.c` の `krn_rom_entries`、`src/array.c`・`src/hash.c`・
  `src/string.c`・`src/range.c`・`src/error.c`・`src/proc.c`、gem では `mruby-metaprog`・
  `mruby-struct`・`mruby-time`・`mruby-io`・`mruby-kernel-ext`。
* **`mrb_define_private_method`**（`src/class.c:1164`）。`mruby-regexp` の
  `__check_initialized`、`mruby-set`/`mruby-data`/`mruby-binding` の `initialize_copy`。
  `mrb_define_module_function_id`（`src/class.c:2790`）は
  「`mrb_define_class_method_id` + `mrb_define_private_method_id`」そのもので、
  **モジュール関数＝特異メソッドは public・インスタンスメソッドは private** というのは
  比喩ではなくこの 2 行である。
* **`mrb_define_method_raw` の中の規則**（`src/class.c:1071-1078`）。
  `initialize`・`initialize_copy`・`respond_to_missing?` の 3 つは、呼び手が何を頼んでも private になる。

### 分かったこと 1: 「いつも private」は組み込みのテーブルには届かない

最初、この 3 つの規則を SabiRuby の `Vm::def_method_raw`（＝`mrb_define_method_raw`）に移した。
すると coverage の食い違いは 50 → 2 になり、残った 2 件が

```
Random#initialize   sabiruby=private  ref=public
Struct#initialize   sabiruby=private  ref=public
```

だった。本家で `initialize` が public になる道があるとは思っていなかったので読み直したところ、
**ROM テーブルは `mrb_define_method_raw` を通らない**。`MRB_MT_INIT_ROM` は
`mrb_mt_init_rom`（`src/class.c:182`）で、const な項目配列をそのままメソッド表の層として繋ぐだけで、
項目の flags をそのまま使う。だから

* 各 ROM テーブルは `initialize` を private にしたければ自分で `| MRB_MT_PRIVATE` と書いている
  （`array.c:2604`、`hash.c:2382`、`string.c:4484`、`range.c:579`、`error.c:916`、`class.c:4662` …）、
* 書き忘れている `mruby-random`（`random.c:610`）と `mruby-struct`（`struct.c:881`）の
  `initialize` は **本家で public のまま**。

つまり「いつも private」は `def`・`alias_method`・`define_method`・C の `mrb_define_method` の規則であって、
組み込みの表の規則ではない。SabiRuby の `vm.define_methods(...)` は ROM テーブルに当たるので、
規則は `Vm::def_method` 側（`OP_DEF`/`OP_TDEF`/`OP_SDEF`・`alias_method`・`define_method`・
`define_singleton_method` が通る道）に残し、組み込み側は本家の表と同じく
「private にしたい項目を自分で挙げる」形にした。`Struct#initialize` と `Random#initialize` は
本家に合わせて public のままにしてある（本家の書き忘れに見えるが、合わせるのが仕事）。

### 分かったこと 2: `Module#method_removed` は本家でも public

`mod_rom_entries` は `method_removed` を `MRB_MT_PRIVATE` で定義している（`class.c:4699`）。
ところが `mruby-metaprog` が `metaprog_mod_rom_entries` で同じ名前を**フラグ無しで定義し直す**
（`metaprog.c:726`）。後から積まれる層が勝つので、既定の gembox で走る本家の
`Module#method_removed` は public。`method_added`・`method_undefined`・`const_added` は private のまま。
SabiRuby でも `ext_metaprog.rs` の `remove_method` の隣に public のまま置いてある。
これは coverage の 50 件に `method_removed` が入っていなかった理由でもあり、
「本家のソースに `MRB_MT_PRIVATE` と書いてある」だけでは答えにならない例。

## 2. 直した場所

`Vm` に 4 つ足した（どれも本家の API にそのまま対応する）。

* `define_private_method` / `define_private_methods` — `mrb_define_private_method`。
  ROM 項目の `| MRB_MT_PRIVATE` に当たる。
* `mark_private(class, &[names])` — 本体が別の一覧に書いてあるときに後から立てる用。
  `Vm::set_visibility` の繰り返しを 1 行にしただけで、それまで 5 か所に手書きされていた
  `let ic = vm.intern("initialize_copy"); vm.set_visibility(...)` を置き換えた。
* `define_module_function` — `mrb_define_module_function`（public な特異 + private なインスタンス）。
* `make_module_function` — 引数付きの `Module#module_function`。既にあるメソッドを
  モジュール関数にする。`kernel.rs` に手書きされていた 11 名のループがこれになった。

内訳（50 件）:

| 種類 | 件数 | どこ |
|---|---:|---|
| `initialize` / `initialize_copy` / `respond_to_missing?` | 16 | `array.rs`・`hash.rs`・`string.rs`・`range.rs`・`exception.rs`・`fiber.rs`・`kernel.rs`・`object.rs`・`ext_binding.rs`・`ext_data.rs`・`ext_regexp.rs` |
| フック（`included`・`extended`・`prepended`・`method_added`・`method_undefined`・`const_added`・`inherited`・`singleton_method_added`/`_removed`/`_undefined`） | 10 | `object.rs` |
| `Module#private`/`public`/`protected`/`module_function`/`remove_const` | 5 | `object.rs` |
| `BasicObject#method_missing` | 1 | `object.rs` |
| `Kernel#__defined_*?` 8 つ | 8 | `kernel.rs` |
| `Regexp#__check_initialized` | 1 | `ext_regexp.rs` |
| モジュール関数（`Complex`・`Rational`・`eval`・`binding`・`sprintf`・`format`・`proc`・`global_variables`・`local_variables`） | 9 | 各 gem の `init` |
| `` Kernel#` `` | 1 | `Vm::load_mrblib` |

`global_variables` と `local_variables` は本家では `mruby-metaprog` のもので、
`metaprog_krn_rom_entries` が private なインスタンス、`metaprog_krn_module_function_entries` が
public な特異、という組。SabiRuby は `global_variables` を `kernel.rs` に書いているので、
`ext_metaprog.rs` が Object のネイティブを Kernel へ移し終えた**後**で 2 つまとめてモジュール関数にしている。

### 捨てた案: `Kernel.\`` を足すこと

`` Kernel#` `` は本家では core の `mrblib/kernel.rb` が public で定義し、`mruby-io` の
`mrblib/kernel.rb` が `module_function def \`` で上書きするので、
測定に使う image では「public な `Kernel.\`` + private なインスタンス」の組になる。
最初はその組をそのまま作った（`make_module_function`）。ところが本家の `test/t/syntax.rb` の
`External command execution.` が落ちた（syntax 70 → 69）。

理由は特異メソッドの側だった。このテストは `module Kernel ... define_method(sym) ... \`test\`` と書く。
`self` が Kernel なので `mrb_class(self)` は **Kernel の特異クラス**で、`` `test` `` は
`define_method` で入れたインスタンスメソッドではなく public な `` Kernel.` `` に当たる。
本家でも同じ経路を通っていることは本家で実測して確かめた（`define_method` が押した `results` が空のまま
「呼ばれた」で返る）。本家がそれで通るのは mruby-io の本体が実際にコマンドを走らせて空文字を返すからで、
SabiRuby の本体は mrblib が与える `NotImplementedError`（no_std に shell は無い）なので落ちる。
特異メソッドの側は項目 10（可視性 50 件）ではなく coverage の別の行
（「モジュール関数の片割れ 3 件」）なので、**インスタンス側を private にするところまでにした**。
`Kernel.\`` は足していない。`global_variables`/`local_variables` の 2 つは足しても
本家テストに影響が無いことを確かめたうえで足してある（「本家だけ」346 → 344）。

## 3. トップレベルの `def`

計画書は「`OP_DEF` → `mrb_vm_define_method`/`mrb_define_method_raw` の可視性が
`mrb_class_ptr(self) == object_class && self == top_self` から選ばれる」規則を想像していたが、
本家にそんな判定は無い。`OP_DEF` は `MRB_METHOD_VDEFAULT_FL`（「場から取れ」）を立てるだけ
（`src/vm.c:4608`）で、`mrb_define_method_raw` が `find_visibility_scope` に聞く。
そして**フレームの既定の可視性はただ 1 か所で private になる**:

```c
/* src/vm.c:119 stack_init */
c->cibase[0] = ci_zero;
c->ci = c->cibase;
c->ci->u.target_class = mrb->object_class;
c->ci->stack = c->stbase;
c->ci->vis = 1;                     /* private (2-bit packed) */
```

文脈（context）の**土台のフレームだけ**が private で始まる。`cipush` が作るフレームは
すべて public（`src/vm.c:868`、`ci->vis = MRB_METHOD_PUBLIC_FL`）。`mrb_top_run` は
`mrb->c->ci > mrb->c->cibase` のときだけ `cipush` するので、

* プログラムの最上段の `def` → 土台のフレーム → **private**、
* 走っている最中に入れ子で回す最上段（`eval("def a5; end")`）→ 積まれたフレーム → **public**。

本家で実測して両方確かめた（`probe2.rb`）。SabiRuby では `run_irep` と `start` が
その土台に当たるので、`Vm::top_vis()` = 「`self.ci` が空なら Private、そうでなければ Public」
の 1 行になった。ブロックの中の `def` は `EnvData` が既に `ci.vis` を写している
（本家の `MRB_ENV_COPY_FLAGS_FROM_CI`）ので、ついてくる。トップレベルで作ったブロックを
Fiber の中で走らせても private なのは、Fiber の文脈ではなく**ブロックを書いた場所**が効くから。

本家と突き合わせた 12 通り（`tests/custom/visibility_toplevel_def.rb`）: トップの `def`、
ブロックの中、Fiber の中、`def` の中の `def`、クラス本体、`Object.class_eval`、`instance_eval` の
特異メソッド、`eval` のトップ、`def self.x`、`class ... def initialize`、`def obj.initialize`、
`define_method`。

### 分かったこと 3: `Module#define_method` は場の可視性を見ない

`tests/custom/method_cache.rb` が落ちて気づいた。トップレベルの
`F.send(:define_method, :v) { ... }` で入れた `v` が private になり、次の `f.v` が
NoMethodError になる。本家の `mrb_mod_define_method_m` は
`define_method_m(mrb, c, MRB_METHOD_PUBLIC_FL)`（`src/class.c:4197`）で、**可視性を書いて渡す**。
`VDEFAULT` ではないので `find_visibility_scope` は呼ばれない。つまり本家では

```ruby
class Foo
  private
  define_method(:dm) { 1 }   # public
  def df; 2; end             # private
end
```

SabiRuby は `current_def_vis` を見ていたので、トップレベルが private になった途端に差が出た。
本家に合わせて `Vis::Public` を書くようにした（`initialize` 系は `Vm::def_method` が
なお private にする）。`module_function` の場でも同じで、`define_method` は
インスタンス側 public・特異側なし。本家で実測（`probe4.rb`）。

トップレベルの `define_method` だけは private で、これは本家が `mrb->top_self` の特異クラスに
`top_define_method`（`define_method_m(mrb, mrb->object_class, MRB_METHOD_PRIVATE_FL)`）を
別に定義しているから（`src/class.c:4209`、`:4766`）。**SabiRuby には main の
`define_method` が無い**（`undefined method 'define_method' for Object`）。
これは「本家にあって無いもの」＝項目 9 の側の話なので足していない。

## 4. `respond_to?` は可視性を見ない

`tests/custom/visibility_respond_to.rb` を本家で作って初めて分かった。本家の `obj_respond_to`
（`src/kernel.c:624`）は `priv` を `mrb_get_args` で受け取るが、**メソッドが見つかったときは使わない**:

```c
mrb_method_t m = mrb_method_search_vm(mrb, &c, id);
if (MRB_METHOD_UNDEF_P(m)) { ... respond_to_missing? ... }
return mrb_bool_value(!MRB_METHOD_NOTIMPL_P(m));
```

引数は `respond_to_missing?` に渡すためだけにある。doc コメントは
`respond_to?(symbol, include_private=false)` と CRuby 風に書いてあるのに、コードは見ていない。
だから本家では `Object.new.respond_to?(:puts)` も `respond_to?(:initialize)` も **true**。
SabiRuby は可視性で弾いていた（`2026-09-16-leftovers-perf.md` の「見つけたが直していない差」）ので、
本家に合わせて弾くのをやめた。`MRB_METHOD_NOTIMPL_P` の false だけ残る。

`send`／`public_send`／`Object#method`／`methods`／`private_methods`／
`instance_methods` 系は本家と同じだった（変更なし）。文言も一致する:
`private method 'top_m' called for Object`、`protected method 'prot' called for Prot`。

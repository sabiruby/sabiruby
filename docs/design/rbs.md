# RBS で境界を宣言する（検討）

`docs/plans/from-mrubyedge-plan.md` の 2 番。ホストが Ruby に出す関数・クラスを RBS で書き、
それをどう使うかを決める段階。実装はしない（試作は `macros/tests/rbs.rs`）。
他の設計文書と違ってこの文書が日本語なのは、`optimizations.md` と同じく「何を選んで何を捨てたか」の
記録が主で、API の説明ではないため。

## 結論

**RBS のパーサは入れない。逆向き（`#[ruby_methods]` から RBS を生成する）だけを進める。**

理由は 4 つ。

1. **読む必要が今のところ無い。** 計画書が挙げた 3 つの用途のうち、パーサが要るのは (a) マクロ時の
   型検査だけで、それは Rust の署名にすでに書いてあることを RBS にもう一度書かせる（二重管理）。
   しかもマクロが見ているのは**型の綴り**であって解決済みの型ではないので、検査できるのは字面の一致だけ。
2. **生成なら二重管理が無い。** `#[ruby_methods]` は引数の型・数・レシーバ・Ruby 名をすでに全部
   知っている。足りないのは戻り値の型を `MethodSpec` に持つこと 1 つ（後述）。
3. **補完の量は組込みクラスにある。** Playground とゲーム内エディタで効くのは `String#gsub` の
   補完であって `Player#damage` ではない。組込みは `define_closure` で定義されていて型が無く
   （`src/` の `define_fn` は 16 件すべて `convert.rs` の rustdoc の中で、実際の定義は 0 件）、
   RBS を持ってきても埋まらない。ここは計画書 3 番（`coverage.md`）と組で「名前だけの補完」が先。
4. **rubevy の `ask` は RBS の問題ではない。** kind ごとの引数と答えがコードを読まないと分からないのは、
   宣言が**どこにも無い**からで、宣言の書式が RBS でないからではない。Rust 側に kind を宣言させれば、
   RBS はその出力の一形式になる。

次の段階の案は最後の節。

## 1. 何に使うか

### (a) `#[ruby_methods]` の生成コードに RBS と Rust の型の一致を検査させる

**要るもの**: RBS のパーサ（proc-macro の中なので `std` は使える）、`.rbs` をマクロから読む手段
（`include_str!` 相当か属性の中に直書き）、Rust の型 → RBS の型の対応表（3 節のもの）。

**得られるもの**: 手で書いた `.rbs` と Rust の署名がずれたとき、コンパイルが落ちる。

**判断: やらない。** 検査の元になる `.rbs` は、Rust の署名から機械的に作れるものを人が書き写したもので、
ずれるのは写した側だけ。つまりこの検査は「写し間違い」しか見つけない。加えてマクロが見ているのは
`syn::Type`、すなわち**綴り**である。`type Hp = i64;` と書いた `fn damage(&mut self, n: Hp)` は、
RBS 側に `Integer` と書いてあっても一致を判定できない（解決された型はマクロ展開の時点では存在しない）。
生成側でも同じ制約があるが、生成なら「知らない綴りは `untyped`」と正直に倒せるのに対し、検査側は
「知らない綴り」を通すか落とすかしかなく、どちらも役に立たない。

### (b) Playground とゲーム内エディタの補完・ホバー文書

**要るもの**: 補完の対象になるクラス・メソッドの一覧と型。消費側は JavaScript（Playground は
CodeMirror 6 を esbuild で 1 本にまとめて持っている、`playground.md`）なので、**Rust の RBS パーサは
要らない**。要るのは JSON なり `.rbs` なりを吐くビルド時の生成器。

**得られるもの**: エディタで `str.` と打ったときの候補と、ホバーの型。ゲーム内エディタでは
ホストが足したクラス（`Player`、`Rubevy::Entity`）も同じ枠で出せる。

**判断: 半分だけ、しかも後で。** 効くのは組込みクラスの補完だが、そこに型は無い。組込みは
`Vm::define_closure` / `define_method` で定義されていて（`src/` に 171 箇所）、引数の型は
`expect_int` などの本体の中にあるだけで署名には出てこない。埋める道は 3 つあるが、どれも今の段階の外:

* CRuby の `core` の RBS（`rbs` gem 同梱）を、計画書 3 番が作る「SabiRuby にあるメソッドの一覧」で
  絞り込む。型は CRuby のものなので mruby の差（`Integer#/` の丸め、`String#slice` の戻り）は嘘になる。
* 組込みを `define_fn` に移す。型が署名に出てくるが、`define_fn` は固定 arity しか扱えない
  （省略可能引数・可変長・キーワードが無い、`convert.rs` の「What it does not do」）ので、
  `String#slice` のような多重定義は移せない。VM の性能にも触る大きな変更で、この検討の範囲外。
* 手で書く。1227 件のテストが通る範囲の組込みを手で RBS にするのは、量的に別の仕事。

したがって v1 は「名前だけの補完」（計画書 3 番の `coverage.md` の生成器がそのまま使える）で、
型が付くのは `#[ruby_methods]` で定義されたホスト側のクラスだけ、という形が現実的。

### (c) rubevy の `Rubevy.ask` の kind ごとの引数と答え

**今**: `kind` は文字列で、引数は `Arg`（`Num(f64)` / `Text(String)` / `Entity(Entity)`）の列、
答えは `Answer`（`Nil` / `Bool` / `Num` / `Text` / `List(Vec<f64>)` / `Rows(Vec<Vec<f64>>)` / `Entity`）。
どの kind に何を渡せて何が返るかは、答える側のシステム（ゲームの `answer_requests`）を読むしかない。
`Rubevy::Proxy` は `respond_to_missing?` が何にでも true を返すので、綴りの間違いは実行時に
「誰も認識しない質問」になる（`rubevy/docs/host-api.md`「A dynamic proxy」）。

**要るもの**: kind ごとの宣言を**ゲーム側**（rubevy ではない）に書かせる場所。書式が RBS なら、
語彙が閉じているので対応は機械的:

| Rust | RBS |
|---|---|
| `Arg::Num` | `Float` |
| `Arg::Text` | `String` |
| `Arg::Entity` | `Rubevy::Entity` |
| `Answer::Nil` | `nil` |
| `Answer::Bool` | `bool` |
| `Answer::Num` | `Float` |
| `Answer::Text` | `String` |
| `Answer::List` | `Array[Float]` |
| `Answer::Rows` | `Array[Array[Float]]` |
| `Answer::Entity` | `Rubevy::Entity` |

```rbs
module Rubevy
  type arg = Float | String | Rubevy::Entity
  type answer = nil | bool | Float | String | Array[Float] | Array[Array[Float]] | Rubevy::Entity
  def self.ask: (String kind, *arg) -> Task::Queue
end
```

ただし語彙は**完全には閉じていない**。`ScriptWorld::answer_value` / `publish_value` はクロージャの中で
VM に直接値を組めるので、答えは `Answer` の 7 つに限らない。rubevy 自身が予約している 4 つの kind
（`component.get` / `component.has` / `components` / `entities.with`、`src/lib.rs:998`）のうち
`component.get` はまさにそれで、Hash を返す。RBS で書くならこの 4 つは

```rbs
# rubevy 自身が答える 4 つ（prelude.rb が送る。ゲームの answer_requests には届かない）
# component.get  : (Rubevy::Entity, String) -> Hash[Symbol, untyped]?
# component.has  : (Rubevy::Entity, String) -> bool
# components     : (Rubevy::Entity) -> Array[String]
# entities.with  : (String) -> Array[Rubevy::Entity]
```

**判断: 価値はあるが、RBS はその答えの一部でしかない。** 今ここに無いのは「型の書式」ではなく
「宣言する場所」で、宣言さえあれば (1) `ScriptWorld` が答えの形を実行時に検査でき、(2) 知らない kind を
質問された時点で（ゲームが無視するのを待たずに）エラーにでき、(3) `Rubevy::Proxy` の
`respond_to_missing?` が本当のことを答えられ、(4) 文書と補完が落ちてくる。RBS はその (4) の出力形式。
これは rubevy の repo の仕事で、SabiRuby 側では何も要らない。

## 2. RBS のパーサ

読む必要が出たときのために、選択肢の現状を記録しておく。

### mruby/edge の `rbs_parser`

`ref/mrubyedge`（`c7dd9ae`、2026-04-14）の `mrubyedge-cli/src/rbs_parser/mod.rs`、328 行。
うちパーサは 236〜328 行の約 90 行（nom 7.1）で、残りは Rust のコード生成。

受け付ける文法は `def NAME: (T, T, …) -> T` **1 形だけ**。

* トップレベルの `def` のみ。`class` / `module` / `end` は文法に無い（`fn_def`、309 行）。
* `def self.foo` は無い。メソッド名は `symbol`（263 行）＝ `[A-Za-z_][A-Za-z0-9_]*` なので、
  `alive?` も `[]` も `<<` も書けない。
* 型は裸の識別子だけ（`ret`、301 行）。`Array[T]`、`T?`、`T | U`、`untyped`、`::Foo`、
  ブロック `{ () -> void }`、キーワード引数、省略可能引数、多重定義、コメント、いずれも無い。
* 意味づけは 4 型のみ。引数側は `Integer` / `Float` / `bool` / `String`、それ以外は
  `unimplemented!("unsupported arg type")` でパニック（20〜29 行、42〜51 行）。戻り側はそれに
  `void` と `SharedMemory` が加わる（83〜95 行）。
* 呼び出し側は `rbs_parser::parse(&s).unwrap()`（`mrubyedge-cli/src/subcommands/wasm.rs:170` と `:195`）
  なので、文法から外れた 1 文字で CLI が落ちる。

用途は `foo.export.rbs` / `foo.import.rbs` から wasm の境界（`#[no_mangle] extern` の並びと
`*const u8, usize` への String の展開）を生成すること（`wasm.rs:152-219`、`mrubyedge/benches/hello.export.rbs`
の中身は `def hello: () -> void` の 1 行）。つまりこれは RBS の部分集合ですらなく、**RBS の見た目をした
wasm の IDL** で、名前が RBS なのは既存の記法を借りただけ（本家の `rbs parse` がこの 1 行を
拒むことは 3 節で確かめた）。SabiRuby が同じものを欲しくなる場面は無い
（SabiRuby は wasm の export を生成していない。Playground は C ABI を手で書いている、`playground.md`）。

なお crates.io の `mrubyedge-cli` は 2.0.1 まで出ており、手元のクローン（1.1.11 相当）より新しい。
上は 2026-04-14 時点の読み。

### crates.io の `rbs` 周り

2026-09-16 に `cargo search` / `cargo info` で確認した。

| crate | 版 | 素性 | 判断 |
|---|---|---|---|
| `rbs` 4.8.4 | — | **無関係**。rbatis（ORM）のシリアライズ基盤 | 名前だけ |
| `ruby-rbs` 0.3.0 (2026-04-03) | BSD-2 | **本家 ruby/rbs 自身**が出している Rust バインディング。33k DL | 後述 |
| `ruby-rbs-sys` 0.3.0 (2026-04-03) | BSD-2 | その FFI 層。rbs の C パーサを `vendor/rbs/` に同梱し、`cc` でビルドして `bindgen` で束ねる | 後述 |
| `tree-sitter-rbs` 0.2.2 (2025-11-13) | MIT | 第三者（joker1007）の tree-sitter 文法。C の parser.c | CST が出るだけで型の AST は無い。エディタ向け |

`ruby-rbs` は 2026-02-03 が初版、0.3.0 が 2026-04-03 で、ruby/prism の `ruby-prism` と同じ
「`config.yml` から `build.rs` でノード型を生成する」作り（`ruby-rbs-0.3.0/build.rs` の冒頭にそう書いてある）。
API は `ruby_rbs::node::parse(&str) -> Result<SignatureNode, String>` と、その下の
`NodeList` / `RBSHash` / `RBSString` / `RBSLocationRange`（`src/node/mod.rs`、751 行）。
本家が出している以上、文法の追随は保証される。

**が、この機械ではビルドできなかった。** 素の crate を 1 つ作って `cargo add ruby-rbs`（依存 42 個）、
`cargo build`:

```
error: failed to run custom build command for `ruby-rbs-sys v0.3.0`
  thread 'main' panicked at bindgen-0.72.1/lib.rs:616:27:
  Unable to find libclang: "couldn't find any valid shared libraries matching:
  ['libclang.so', 'libclang-*.so', ...], set the `LIBCLANG_PATH` environment variable ..."
```

C コンパイラ（`cc`）と libclang（`bindgen`）がビルド時に要る。SabiRuby の側から見ると、これは

* VM の crate（`no_std` + alloc、依存 4 つ）には**絶対に入れられない**。
* wasm32 / thumbv7em のビルドにも入れられない。
* CI と、この repo を clone した人の機械に libclang を要求する。

ので、入れるとしてもホスト側の開発ツール（`tools/` の別 crate、あるいは workspace の
`[[bin]]` を dev 専用 feature の裏に置く）に限る。**読む必要が出るまで入れない**という結論を、
この実測が後押ししている。

### 手書きの部分集合パーサ

(a) の検査に要る範囲（`class`、`def`、`def self.`、`?` 付きのメソッド名、`Integer` / `String` / `bool` /
`Float` / `Symbol` / `untyped` / `nil` / `void` / `T?` / `Array[T]` / タプル）なら、150〜250 行で書ける。
mruby/edge のものより素直に大きくなるのは、SabiRuby が扱いたい型が `Option` と `Vec` を持つため。

**が、部分集合のパーサは「受け付ける」ことで嘘をつく。** RBS を知っている人は `String | Symbol` や
`(Integer, ?Integer) -> String` を書くし、それが通らないと「RBS が書ける」と言った側の落ち度になる。
本家のパーサが crate で手に入る以上（libclang 付きだが）、部分集合を自分で持つ理由は「依存を増やさない」
だけで、その依存が要るのは検査をやる場合だけ——そして検査はやらない。

## 3. 逆向き: `#[ruby_methods]` から RBS を生成する

### 対応表

`src/convert.rs` に今ある `FromRuby` / `IntoRuby` の実装をすべて並べたもの。「引数」列が空なのは
`FromRuby` が無い型（＝答えにしか出られない）。

| Rust | 引数 (`FromRuby`) | 答え (`IntoRuby`) | RBS |
|---|---|---|---|
| `i64` | ✓ | ✓ | `Integer` |
| `i32` | ✓（範囲外は `RangeError`） | ✓ | `Integer` |
| `i8` `i16` `u8` `u16` `u32` | — | ✓ | `Integer` |
| `u64` `usize` | — | ✓（`i64` に入らなければ BigInt） | `Integer` |
| `f64` | ✓（Integer は広がる） | ✓ | `Float` |
| `f32` | — | ✓ | `Float` |
| `bool` | ✓（nil と false だけが偽。raise しない） | ✓ | `bool` |
| `String` | ✓（UTF-8 に lossy） | ✓ | `String` |
| `&str` | — | ✓ | `String` |
| `Bytes` | ✓（バイト列のまま） | ✓（String を作る） | `String` |
| `Sym` | ✓ | ✓ | `Symbol` |
| `Value` | ✓（素通し） | ✓ | `untyped` |
| `DataRef` | ✓（`Data` でなければ `TypeError`） | — | `untyped` |
| `Option<T>` | ✓（nil が `None`） | ✓（`None` が nil） | `T?` |
| `Vec<T>` | ✓（Array を要素ごと） | ✓（Array） | `Array[T]` |
| `(A, B)` … `(A..F)` | — | ✓（Array） | `[A, B]` …（RBS のタプル） |
| `()` | — | ✓（nil） | 戻り値なら `void`、引数位置なら `nil` |
| `This<T>` | 文脈 | — | レシーバ。引数の並びから消える |
| `Block` | 文脈 | — | `{ (*untyped) -> untyped }`（arity が分からない） |
| `&mut Vm` | 文脈 | — | 何にもならない |
| `Self` / `#[derive(RubyClass)]` した型 | — | ✓（store に入れてハンドルを返す） | そのクラスの Ruby 名 |
| `Result<T, VmError>` | — | ✓（再 raise） | `T`（RBS は例外を書かない） |
| `Result<T, String>` / `Result<T, &'static str>` | — | ✓（`RuntimeError`） | `T` |
| `Serde<T>`（`sabiruby-serde`） | ✓ | ✓ | `untyped`（下記） |

`Serde<T>` を `{ hp: Integer, name: String }` のようなレコード型にできないのは、serde のデータモデルが
**属性で変わる**ため。`#[serde(rename)]` / `skip` / `flatten` / `tag` / `default` は `sabiruby-macros` から
見えず、struct のフィールドをそのまま写すと嘘になる。正しくやるなら serde の derive 側（＝`sabiruby-serde` の
中に derive を足す）から出すしかなく、それは別の crate の別の仕事。v1 は `untyped`。

### マクロに見えるもの・見えないもの

`macros/src/expand.rs` を読んだ限り、展開時に手元にあるのは:

* Ruby のメソッド名（`MethodSpec.ruby_name`、`#[ruby(name = "alive?")]` 込み）と `#[ruby(skip)]`。
* レシーバの種別（`Recv::None` / `Ref` / `Mut`）＝ クラスメソッドかインスタンスメソッドか。
* 先頭の `&mut Vm` が文脈であること（`is_vm`、258 行）。
* 引数の型（`MethodSpec.args: Vec<(Ident, Type)>`）と、その `syn::Pat` にある**引数名**。

足りないのは 2 つだけ:

1. **戻り値の型。** `method_spec` は `sig.output` を読んでいない（`define` が
   `IntoRubyRet` に投げるだけで済むため）。`MethodSpec` に `ret: syn::ReturnType` を 1 つ足せば済む。
2. **Ruby のクラス名。** `#[ruby(name = "Monster")]` は struct の `#[derive(RubyClass)]` の側にあり、
   `#[ruby_methods]` の `impl Beast` からは見えない。実行時なら
   `<Beast as RubyClass>::NAME` で取れるので、これは出力先の選び方（下）で決まる。

そして**解決済みの型は見えない**。`n: Hp`（`type Hp = i64`）は `Hp` という綴りでしかない。
生成器はこれを `untyped` にする——`Integer` と嘘をつかず、コンパイルも落とさない。同じ理由で、
答えに現れる他のホストクラス（`fn spawn(&self) -> Beast`）も `untyped` になる。`Beast` の Ruby 名は
`Monster` かもしれず、綴りから当てられない。`Self` だけは当てられる。

### 出力先の 3 案

| 案 | 仕組み | 良い点 | 悪い点 |
|---|---|---|---|
| **A. 実行時の定数** | マクロが `impl T { pub const RBS: &str = "…"; }` を生やし、ホストが集めて書き出す | クラス名を `RubyClass::NAME` から取れる。ビルドに副作用が無い。Playground は VM から直接引ける | 生成物を得るのに一度プログラムを走らせる |
| B. マクロがファイルを書く | 展開中に `std::fs::write` | 手数ゼロ | 出力先が決まらない（proc-macro に `OUT_DIR` は無い）。`cargo check` でも走る。増分ビルドのキャッシュを壊す。サンドボックスで落ちる |
| C. 外部ツール | `syn` でソースを読む `tools/` の bin | マクロに触らない | `#[ruby_methods]` の解釈が 2 か所になる（ずれる） |

**A を推す。** 文字列 1 本が増えるだけ（`Player` で 300 バイト弱）で、要らないホストのために
feature（既定オフ）の裏に置ける。Playground とゲーム内エディタは VM の中から
`Player::RBS` を読めるので、ファイルを配る必要すら無い。

### 試作

`macros/tests/rbs.rs`。使い捨てで、`src/` には何も足していない（proc-macro crate は
マクロ以外を公開できないので、置く場所が無い、という事情もある）。`syn` で `impl` ブロックを読み、
上の対応表どおりに RBS を書く 150 行ほど。`cargo test --workspace` で回る。

`macros/tests/player.rs` の `Player`（`#[ruby(name = "alive?")]` と `#[ruby(skip)]` 込み）を入れると:

```rbs
class Player
  def self.new: (Integer hp) -> Player
  def self.strongest: (Integer a, Integer b) -> Player
  def hp: () -> Integer
  def damage: (Integer n) -> void
  def rename: (String to) -> String
  def name: () -> String
  def alive?: () -> bool
  def greet: (untyped other) -> String
end
```

`self.` が付いたのは `self` を取らない `fn` で、`strongest` の `&mut Vm` と `greet` の `&mut Vm` は
引数から消え、`greet` の `VmResult<String>` は `String` に、`damage` の戻り値なしは `void` に、
`secret` は `skip` で出ていない。`other: Value` が `untyped`。引数名（`hp`、`n`、`to`）は
Rust の引数名がそのまま使えた。

対応表の残りも同じ試作で確かめてある:

```rbs
class Shapes
  def floats: (Float a, Float b) -> Float
  def maybe: (Integer? n) -> String?
  def list: (Array[Integer] xs) -> Array[Array[Float]]
  def nested: (Array[Integer?] xs) -> Array[String]?
  def bytes: (String b) -> String
  def sym: (Symbol s) -> Symbol
  def raw: (untyped v) -> untyped
  def handle: (untyped d) -> untyped
  def pair: () -> [Integer, String]
  def borrowed: () -> String
  def fallible: () -> Integer
  def nothing: () -> void
  def config: (untyped c) -> untyped
  def unknown: (untyped w) -> untyped
end
```

**出力が本物の RBS であることは本家の `rbs` gem で確かめた**（3.6.1、この機械に入っていた）。
上の 2 つを `.rbs` に落として

```
$ rbs parse player.rbs shapes.rbs     # 何も言わずに exit 0
$ rbs -I . validate                   # 同じく
```

ついでに分かったこと: mruby/edge の `rbs_parser` はこの出力を 1 行目（`class Player`）で拒む。
そして**逆も通らない**。mruby/edge の `hello.export.rbs` の中身をそのまま本家に渡すと:

```
$ rbs parse toplevel.rbs
toplevel.rbs:1:0...1:3: Syntax error: cannot start a declaration, token=`def` (kDEF)
  def hello: () -> void
  ^^^
```

RBS でトップレベルに書けるのは宣言（`class` / `module` / `interface` / 定数 / `type`）であって、
`def` は `class …  end` の中にしか置けない。つまり mruby/edge の `.rbs` は RBS の部分集合ですらなく、
**RBS の見た目をした別の記法**である。`ruby-rbs` を入れれば mruby/edge と同じことができる、とは
ならない（できるが、`.rbs` の書き方が変わる）。

## 4. 次の段階の案

**到達点**: `#[ruby_methods]` が、その `impl` ブロックの RBS を `T::RBS` として生やす。
`Player::RBS` を集めて `.rbs` に落とせば `rbs validate` が通る。

**やること**:

* `macros/src/expand.rs`: `MethodSpec` に `ret: syn::ReturnType` を足し、試作の `rbs_type` /
  `rbs_return` を移す。`Self` はクラス名に、知らない綴りは `untyped`。
* クラス名は `const RBS` の中に `{}` を開けておき、`register` ではなく `RubyClass::NAME` を使って
  実行時に埋める（`concat!` では届かないので `RBS` は `fn rbs() -> String` にするか、
  `class {NAME}` の行だけ実行時に作る）。ここは実装で決める。
* feature（既定オフ）の裏。要らないホストに 1 バイトも払わせない。
* `macros/tests/rbs.rs` を、試作から「生成されたものの検査」に書き換える。
* `docs/design/macros.md` に 1 節、この文書へのリンク。

**やらないこと**: RBS を読むこと（パーサを入れない）。組込みクラスの RBS。`Serde<T>` のレコード型。

**大きさ**: 小。

**その先**（別の段階、別の repo）:

* rubevy: `ask` の kind の宣言を `ScriptWorld` に持たせ、その出力の 1 つとして RBS。
  1 節 (c) の表がそのまま使える。rubevy の計画書に置くべき項目。
* Playground: 計画書 3 番の `coverage.md` の生成器から「名前だけの補完」。型が付くのは
  ホストのクラスだけ、と最初から言い切る。

## 5. 著者判断（2026-09-17）: パーサは入れない、`.rbs` を出荷する

0.5.0 の準備にあたって著者が決めた。**VM に RBS のパーサは入れない。** 代わりに、
**`.rbs` の署名そのものを出荷物として持つ**（`sig/`）。読む側は Steep / TypeProf / エディタの LSP が
既に持っているので、こちらが書くべきなのは「何があるか」を RBS で言うことだけである、という切り分けで、
1 節で挙げた 3 つの用途のうち「実行時に型を検査する」を捨て、「書くときに補完と検査が効く」を採る。

`sig/` に入るものは 2 種類ある。**生成するもの**が `#[ruby_methods]` から出る `T::RBS`
（4 節の設計そのまま。ホストが定義したクラスは、綴りから型が分かる唯一の場所である）と、
`docs/verification/coverage.md` の生成器から出る組込みクラスの一覧。**手で書くもの**が、
生成器では型まで分からない組込みメソッドの署名と、ホスト側の入口
（rubevy の `Rubevy` / `Entity` / `Proxy` / `ask` の kind、rubevy_games の DSL）である。
`docs/plans/from-mrubyedge-plan.md` の 2b 行がこの項目で、状態は「未着手（著者判断 2026-09-17）」。

3 節で見たとおり mruby/edge の `.rbs` は RBS の部分集合ですらないので、**そちらに寄せない**ことも
同時に決まっている。こちらが出す `.rbs` は本家 `rbs validate` が通るものであり、そうでなければ
Steep も TypeProf も読めない——「読む側を持たない」という判断は、「書くものが本物の RBS である」
ことと対になっている。

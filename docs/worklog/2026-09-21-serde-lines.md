# 2026-09-21 — 宣言の表が行を覚える／map のキーを Symbol で返す（`serde-declare-lines-plan.md` D1〜D3）

計画書は [`docs/plans/serde-declare-lines-plan.md`](../plans/serde-declare-lines-plan.md)。
作業場所は worktree `sabiruby-wt-serde-lines`（ブランチ `serde-lines`、先頭は `87e5aaf`）。
出どころは rubevy_games の Factory F2 で、利用者の側の実物は `factory/src/data.rs` の
`line_of`（宣言の行をソースから探す 10 行の迂回）と、651 行あたりの「map のキーは String で返る」
という注意書き。

## 0. 着手前の数

```
$ cargo test --workspace            # 237 passed; 0 failed（test result の 4 列目を合計）
$ ./tools/check_no_std.sh           # no_std OK
$ cargo build -p sabiruby-serde --lib --no-default-features --target thumbv7em-none-eabi
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.17s
```

`tools/check_no_std.sh` は VM の crate しか見ない（`host-scale-plan` の H6 がそれを直す段階）ので、
serde の crate は `docs/design/serde.md` の末尾が書いている thumbv7em の直接ビルドで見る。
これは通る道があったので「確かめる道が無い」とは書かない。

## 1. 着手前に確かめた 2 つのこと

計画書は D1・D2 それぞれの設計の理由を、実物の振る舞いに賭けている。先に確かめた。
確かめる仕掛けは `serde/tests/zz_probe.rs` に書いて、答えを取ったあと消した（本物のテストは
D1・D2 の節に書いたものが受け持つ）。

### 1.1 `Vm::intern` の Symbol の表は GC で回収されるか → **されない**

`src/symbol.rs` の `Interner` は `names: Vec<Box<[u8]>>` と `index: HashMap<Box<[u8]>, Sym>` の
2 つだけで、**要素を取り除くメソッドが 1 つも無い**（`intern` / `intern_str` / `lookup_str` /
`name` / `name_str` / `len` / `is_empty`）。`Sym(u32)` は `names` の添字そのもので、
「every symbol is an index into the interner」と型の rustdoc が言っている以上、
途中を抜けば残り全部の番号がずれる。`docs/design/gc.md:61` の表も「symbols | never collected」と
既に書いている。

読むだけでは足りないので測った:

```
syms: before=1014 after_intern=2014 after_gc=2014
script syms: before=1014 after=3014
```

1 行目は Rust から `vm.intern("probe_symbol_{i}")` を 1000 回、そのあと `gc_collect()` を 2 回。
1014 → 2014 に増えて、collect を 2 回通しても 2014 のまま。2 行目は Ruby 側から
`2000.times { |i| "ruby_sym_#{i}".to_sym }` のあと `GC.start` と `gc_collect()`、1014 → 3014 で
やはり 1 つも戻らない。**回収されない。**

したがって計画書 D1 の理由はそのまま立つ: map のキーは実行時の値で数に上限が無く、それを
無条件に intern するのは「表が一方通行で伸びる」ことを利用者に黙って選ばせることになる。
だから `symbol_keys` に相乗りさせず、別のスイッチ（`symbol_map_keys`、既定 `false`）にする。
`expose` が `Options::symbols()` を使ってよいのは、そこで intern される名前が**宣言の名前**、
つまり宣言の数しか無く、しかも Ruby の側で既に Symbol として書かれている（= もう intern 済みの）
ものだから。新しい Symbol は 1 つも増えない。

### 1.2 native の中で `Vm::current_line()` は何を返すか → **呼び出しの次の命令の行**

`Vm::current_line()`（`src/vm.rs:2621`）は `self.ireps[ci.irep].line_of(ci.pc)` である。
一方 `Vm::backtrace`（同 2560 あたり）の `loc` は **`ci.pc.saturating_sub(1)`** を引いていて、
コメントに「`ci.pc` is past the instruction being executed」と書いてある。実行ループは
オペランドを読み終えた時点で `self.ci[top].pc = pc`（`src/vm.rs:3443`）と、**次の命令を指す pc** を
フレームに書いてから命令を実行する。native（`define_closure`）は `call_closure_direct` が呼ぶだけで
`ci` を積まない（`src/vm.rs:2282`）ので、native の中から見える innermost フレームは呼び出し元の
Ruby のフレームであり、その `pc` は **SEND の次の命令**を指している。

測った（`unit` を `define_closure` で置いて、中から両方を印刷した）:

```
current_line=Some(2) pc-1=Some(1) ci=Some((537, 12)) backtrace=["data.rb:1"]
current_line=Some(4) pc-1=Some(3) ci=Some((537, 24)) backtrace=["data.rb:3"]
current_line=Some(5) pc-1=Some(4) ci=Some((537, 36)) backtrace=["data.rb:4"]
```

ソースは `unit :a, x: 1` / `unit :b,` + `     x: 2` / `unit :c, x: 3` / `$z = 1`。
**`current_line()` は 1 行ずつ後ろを指している。** 2 行にまたがる宣言でも同じずれ方で、
backtrace が 3（= 宣言が終わる行、`SEND` の行）と言うところを 4 と言う。
末尾の宣言だけは偶然一致する（次の命令が同じ行の `RETURN` なので）:

```
tail: current_line=Some(2) backtrace=["tail.rb:1"]
tail: current_line=Some(3) backtrace=["tail.rb:3"]
```

デバッグ情報の無い `.mrb` では両方とも空:

```
nodebug: current_line=None backtrace=[]
```

**`current_line()` は直せない。** sabiruby-playground の `sabi_step_until`
（`wasm/src/lib.rs:316` と `324`）がステップ実行の「次に実行する行」としてこれを使っていて、
命令の境界で止まったデバッガが欲しいのはまさに `line_of(pc)` の方である。意味が違う 2 つで、
どちらも正しい。だから**既存の利用者の振る舞いを変えず、VM に safe な口を 1 つ足した**
（D2 の節）。

## 2. D1 — `Options::symbol_map_keys`

### 2.1 `#[non_exhaustive]` と「`..Default::default()` に寄せる」— 計画書の 1 行が成り立たない

計画書は「`#[non_exhaustive]` を付け、作り方をコンストラクタと `..Default::default()` に寄せる」と
書いている。**後半は成り立たない。** Rust の `#[non_exhaustive]` は定義した crate の外で
struct expression を禁じ、それには**関数更新構文（FRU）も含まれる**ので、外の利用者は
`Options { symbol_keys: true, ..Default::default() }` とも書けない
（[Reference, "Struct expressions"](https://doc.rust-lang.org/reference/expressions/struct-expr.html):
"cannot be used outside the defining crate"）。

狙い（**後からフィールドを足しても破壊的変更にならない**）はそのままなので、作り方を

1. コンストラクタ（`Options::default()`、`Options::symbol_keys()`、`Options::symbols()`）で作り、
2. 足りなければ**公開フィールドに代入する**（`non_exhaustive` は構築を禁じるだけで、
   公開フィールドの読み書きは禁じない）

の 2 段にした。rustdoc に 3 行の例でそう書いてある。設計の分かれ道ではなく計画書の
言い回しの訂正なので、原則（`CLAUDE.md`「小さな判断は原則で決める」）に照らしてここで決めた。

### 2.2 どこで Symbol にするか — 「型が文字列」ではなく「**値が String になった**」で判定した

`Serializer::key`（`ser.rs:36`）は struct のフィールド名と variant 名にしか通っていない。
map のキーは `MapSer::serialize_key` が普通の値として書く。2 つの道を考えた:

1. **キー専用の `Serializer` を被せる。** `serialize_str` だけを差し替えた serde の
   `Serializer` を書き、`serialize_key` でそれに渡す。「文字列として serialize された」を
   型の側で正確に捉えられるが、serde の `Serializer` は 30 近いメソッドがあり、
   転送の定型が 150 行ほど増える。
2. **書けた `Value` を見る。** 普通に serialize して、出てきたのが String だったら Symbol にする
   （`Serializer::map_key`）。

2 を採った。`ser.rs` の `MapSer` の rustdoc が既に「The keys are whatever the key type
serializes to」と、**キーは値として決まる**と言っている crate だからで、`String` / `&str` /
`char` / それらを包んだ newtype・`Some` が全部同じところに落ちるのは、利用者が「キーが文字列で
書かれるとき」と言うときに意味しているものそのものである。判定は 1 か所（`serialize_key` の
直後）なので、**複合キーの中の文字列は触らない** — `BTreeMap<(String, i64), _>` のキーは
`["a", 1]` のままで、テストがそれを押さえている。

除くものを 2 つ入れた: **UTF-8 でないバイト列**（`Vm::intern` が `&str` しか取らないので
Symbol になりようがない）と、**binary の印が付いた String**（`serialize_bytes` の落ち先。
バイト列は名前ではない — この crate がテキストを読むところで引いているのと同じ線）。

### 2.3 外に `Options { .. }` を書いている利用者はいない

```
$ grep -rn "Options *{" --include=*.rs rubevy rubevy_games sabiruby-playground mruby-porting-kit \
    | grep -v "sabiruby_compiler::Options"
（1 件も無い）
```

`Options {` の綴りは 20 件ほど当たるが**全部 `sabiruby_compiler::Options`**（別の型、
`filename` と `debug_info` を持つコンパイラの設定）である。`sabiruby_serde::Options` を
名指しているのは rubevy_games の `factory/src/data.rs:651` の**注意書きの散文 1 か所**だけで、
コードではない。したがって `#[non_exhaustive]` を付けても今のところ誰も壊れない。

### 2.4 テスト

`serde/tests/roundtrip.rs` に 4 つ（`Recipe { ingredients: BTreeMap<String, u32>, time: f64 }` を
題材に）:

* `a_maps_keys_are_strings_under_symbol_keys_as_they_always_were` — **前の振る舞いが変わっていない**。
  `Options::symbol_keys()` の下では `{ingredients: {"coal" => 2, "iron_ore" => 1}, time: 2.0}`。
* `symbols_reaches_a_maps_keys_too` — `Options::symbols()` で `{ingredients: {coal: 2, iron_ore: 1}, time: 2.0}`。
* `a_hash_a_script_wrote_with_symbol_keys_comes_back_the_way_it_was_written` — **往復**。
  Ruby で `$was = {iron_ore: 1, coal: 2}` と書き、`BTreeMap<String, u32>` に読み、
  `Options::symbols()` で書き戻して Ruby の `==` で比べる（`true`）。`symbol_keys()` で書き戻すと
  `false`、というのが直した穴そのもの。
* `only_a_key_that_serializes_as_a_string_becomes_a_symbol` — 整数のキー、複合キー、binary の
  String がそれぞれ触られない。

`serde/tests/declare.rs` に 1 つ、`the_names_inside_a_published_entry_read_back_as_the_symbols_they_were_written_as`:
`expose` した `recipe_of(:iron_plate)` が `{ingredients: {iron_ore: 1}, time: 2.0}` で、
`[:ingredients][:iron_ore]` が引ける。これが Factory が欲しかった綴り。

```
$ cargo test -p sabiruby-serde
declare   15 passed（14 → +1）
json       6 passed
roundtrip 15 passed（11 → +4）
doc        6 passed（5 → +1、`Options` の例）
```

## 3. D2 — `Declared<T>` と `take_with_lines`

（作業しながら追記）

## 4. D3 — docs

（作業しながら追記）

## 5. 気づいた点

（作業しながら追記）

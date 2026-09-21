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

### 3.1 `current_line()` では届かないので VM に口を 1 つ足した — `Vm::backtrace_line()`

§1.2 で測ったとおり `current_line()` は 1 命令先を指す。足さずに済む道を 3 つ考えた:

1. **`current_line()` を直す**（`line_of(ci.pc - 1)` にする）。**やらない。**
   sabiruby-playground の `sabi_step_until` がステップ実行の「次に実行する行」としてこれを
   読んでいて（`wasm/src/lib.rs:316`・`324`）、そこでは `line_of(pc)` の方が正しい。
   既存の利用者の振る舞いを黙って変える変更で、しかも「どちらかが間違っている」のではなく
   **意味の違う 2 つの問い**である。
2. **serde から `vm.backtrace(None)` を呼んで先頭の文字列の `:N` を読む。** 文字列を組み立てて
   また解くうえ、ファイル名にコロンが入りうるので切る場所が形頼みになる（Factory が
   `place_in` で既に踏んだ穴と同じ）。
3. **serde から `vm.ci` と `vm.ireps` を直に読む。** どちらも `pub` なので**書ける**が、
   別 crate から VM の内部表現に手を入れることになり、`backtrace` と同じ計算を 2 か所に
   持つことになる（`pc - 1` の引き算を忘れた方が静かに 1 行ずれる、まさにこの問題）。

どれも良くないので、**VM に safe な読み取りを 1 つ足した**（計画書が許している）。unsafe は無し、
`no_std` のまま、`&self` だけ:

```rust
/// The line the first frame of [`Vm::backtrace`] carries: where the instruction now
/// running is. From inside a native — a `define_fn` or `define_closure` method, which
/// pushes no frame of its own — that is the line of the call, so a native that records
/// this and a native that raises name the same line.
pub fn backtrace_line(&self) -> Option<u32> {
    for ci in self.ci.iter().rev() {
        let Some(ir) = self.ireps.get(ci.irep) else { continue };
        if ir.lines.is_empty() { continue; }
        return ir.line_of(ci.pc.saturating_sub(1));
    }
    None
}
```

`backtrace` の `loc` と同じ走り方（デバッグ情報の無いフレームは飛ばす）にしてある。
違うのは 1 点だけで、`backtrace` が置き場所を決めかねて `file:0` と書く場合をここでは `None` に
している。`current_line()` の rustdoc にも「これは**次に実行する**命令の行で、エラーの行が
欲しければ `backtrace_line` を見よ」と書き足した。**振る舞いは 1 つも変えていない。**

VM 側のテストは `tests/native.rs` に 2 つ:
`a_native_asks_backtrace_line_for_the_line_it_was_called_from`（1 行の呼び出しと 2 行に
またがる呼び出しで、`backtrace_line` が `[Some(1), Some(3)]`、`backtrace(None)` の先頭が
同じ `(test):1` と `(test):3`、`current_line` は 1 つ先の `[Some(2), Some(4)]`）と
`without_debug_information_there_is_no_line_and_no_backtrace`（`(None, 0)`）。

### 3.2 `Declared<T>` と、控える場所を 1 つにする

```rust
#[non_exhaustive]
pub struct Declared<T> { pub name: String, pub value: T, pub line: Option<u32> }
```

`Collected<T>` の `order` を `Vec<(String, T)>` から `Vec<Declared<T>>` に替えた。
`take_with_lines` はそれをそのまま返し、**`take` はその上に 1 行で書いた**
（`.map(|d| (d.name, d.value))`）ので、控える場所は `define_on` のクロージャの 1 か所きり。
返り値の型は変えていないので今の利用者（rubevy_games の Factory）はそのまま建つ。

行を取るのは**クロージャの先頭、`check_argc` の直後**である。native はフレームを積まないので
そこでの innermost フレームは呼び出し元の Ruby のフレームそのもので、この先で
`from_value` が Ruby を走らせうる（キーの `hash`/`eql?`）より前に読んでおく必要がある。
`define_replacing` の上書きは `t.order[i].value` と `t.order[i].line` を両方書き替える
（順番の位置は動かさない）: ホストが持っている値が書かれた行は**後の宣言の行**だから。

### 3.3 テスト — 「同じ数」を同じ入力で見せる

計画書が求めているのは「2 行にまたがる宣言で、わざと型を間違えた版が raise で言う行と、
正しい版が控えた行が一致する」こと。テストは **1 つのソース生成関数**（`over_two_lines(scale)`、
`scale:` に渡すものだけが違う）から 2 本の走行を作り、**数を書かずに突き合わせている**:

```rust
let e = run(&mut vm, "data.rb", &over_two_lines("\"not a number\"")).expect_err(…);
let complained_at = raised_at(&mut vm, &e);      // backtrace の先頭フレームの行
assert_eq!(complained_at, 4);
…
assert_eq!(table.iter().map(|d| d.line).collect::<Vec<_>>(),
    [Some(2), Some(complained_at), Some(5)]);
```

`unit :inch,` が 3 行目、`     symbol: "in", scale: …` が 4 行目で、**どちらも 4 と言う**
（宣言が終わる行、`SEND` が持つ行）。Factory の `line_of` は同じ宣言に 3 を返していた
（先頭の語で探すので「始まる行」）ので、迂回を消すと**その 1 件だけ数が変わる**（§5 の 2）。

ほかに 3 つ: `take_is_take_with_lines_without_the_lines`（古い口が前と同じに答え、
同じように表を空にする）、`an_amended_declaration_keeps_its_place_but_takes_the_later_line`
（`[Some(3), Some(2)]` — `metre` は 1 行目で宣言され 3 行目で上書きされたので 3）、
`a_declaration_from_bytecode_without_debug_information_has_no_line`（`None`）。

```
$ cargo test --workspace
TOTAL passed: 250  failed: 0        （243 → +7）
$ ./tools/check_no_std.sh            → no_std OK
$ cargo build -p sabiruby-serde --lib --no-default-features --target thumbv7em-none-eabi
                                     → Finished
$ ./target/release/sabiruby mrbtest tests/mrbtest/assert.mrb …   # 本家テスト、VM に触ったので
$ diff tests/mrbtest/baseline.txt <(…)  → BASELINE IDENTICAL
```

本家テストは Docker を使う `tools/mrbtest.sh` ではなく、**チェックインされている
`tests/mrbtest/*.mrb` を `target/release/sabiruby mrbtest` で回して baseline と比べた**
（`implementer.md` が書いている代替の道）。`mrbtest.sh` は `../../ref/mruby` から
ソースを取り直して `src/mrblib/*.mrb` と `docs/verification/mrbtest.md` を書き替えるので、
この仕事と無関係な差分が出る。Docker 自体は動いていた（`docker info` は通る）が、起動も
再起動もしていない。

## 4. D3 — docs と、版をどうするか

直したもの:

* `docs/design/serde.md` — データモデルの節に「Symbols, and which of them are an option」を
  新設（3 つの `Options` の表と、なぜ 2 つのスイッチなのかの理由 = §1.1 の測定）、
  `declare` の節に「The line, for the checks serde cannot do」（`Declared<T>`、
  `current_line` との違い）、`expose` の節に「Symbols が下まで届く」、
  「What the VM gained for this」に `Vm::backtrace_line`（と、足さずに済ませる 3 案がそれぞれ
  何を払うことになったか）、「Where it is checked」に増えたテスト。
* `serde/src/lib.rs` の頭の表（`map` の行と `Options::symbols` への言及）と
  「# Declarations」の節。
* `docs/README.md` の worklog の目次にこの記録。plans の表の行は本体が更新する（計画書の §3 は
  この段階で埋めた）。
* `CHANGELOG.md` の Unreleased。**ここは黙って嘘になっていた**: 「the VM … untouched」と
  書いてあったが `Vm::backtrace_line` を足したので、`sabiruby-serde` の 2 件と `sabiruby` の
  1 件に書き分けた。版は上げていない。

**版は上げない**（公開は著者の仕事）。上げるとすればこうなる、というのが報告用の答え:

| crate | 今 | 上げるなら | なぜ |
|---|---|---|---|
| `sabiruby-serde` | 0.1.0 | **0.2.0** | `Options` に `#[non_exhaustive]` を付けたのは破壊的変更（外の crate で struct リテラルが書けなくなる）。0.x では minor を上げるのが破壊的変更の作法 |
| `sabiruby` | 0.5.2 | **0.5.3** | `Vm::backtrace_line` は追加のみ、振る舞いの変更なし |
| `sabiruby-compiler` / `-cli` / `-macros` | — | 据え置き | 触っていない |

### 4.1 rubevy と rubevy_games が今の main でそのまま建つか → **建つ**

どちらの repo にも 1 バイトも書かずに確かめた。手順:

```bash
git -C rubevy_games archive HEAD | tar -x -C <scratchpad>/dl/games   # 作業木ではなく HEAD
git -C rubevy       archive HEAD | tar -x -C <scratchpad>/dl/rubevy
# コピーした木の [patch.crates-io] だけをこのブランチの worktree に向ける
CARGO_TARGET_DIR=<scratchpad>/t-games cargo check --workspace --all-targets
```

**コマンドラインの `--config "patch.\"https://github.com/sabiruby/sabiruby\".sabiruby.path=…"` は
効かなかった。** rubevy_games は既に `[patch.crates-io] sabiruby = { git = … }` を持っていて、
その git ソースをさらに patch しようとすると `Cargo.lock` の固定（`#50cae754`）と衝突して

```
error: failed to select a version for `sabiruby`.
    ... previously selected package `sabiruby v0.5.2 (https://github.com/sabiruby/sabiruby#50cae754)`
```

と断られる。指示が用意していた逃げ道（「`Cargo.lock` が書き換わるなら `CARGO_TARGET_DIR` を
別にした上でコピーした木で」）に落として、**`git archive HEAD` で出した汚れていない木**の
`[patch.crates-io]` を path に書き替えた。repo 自体は読むだけ（`rubevy` の作業木には
別の担当の未コミットの `Cargo.toml` の変更が 6 行あったので、なおさら `HEAD` から出す意味があった）。

結果:

```
=== rubevy check ===
    Checking rubevy v0.0.1 (…/dl/rubevy)
    Finished `dev` profile … in 21.48s
rubevy exit=0
=== games check ===
   Compiling factory v0.1.0 (…/dl/games/factory)
    Checking rubevy v0.0.1 (https://github.com/sabiruby/rubevy#abfc875f)
    Checking games-shell v0.1.0 (…/dl/games/crates/games-shell)
    Finished `dev` profile … in 2m 47s
games exit=0
```

`cargo metadata` が `Adding sabiruby v0.5.2 (/home/kishima/book/kishima/sabiruby-wt-serde-lines)` と
言っているので、見ているのは確かにこのブランチである。games の方は `rubevy` を GitHub の
main（`abfc875f`）から引くので、**rubevy の main も依存として一緒に通っている**。
`--all-targets` なのでテストのターゲットもコンパイルされている。

### 4.2 Factory の単体テストを走らせたら、**狙いどおり 1 件だけ落ちた**

建つことは分かったので、ついでに `cargo test -p factory --bins` を同じコピーで走らせた
（`factory` は lib を持たないので `--bins`）。**33 passed, 1 failed:**

```
---- data::tests::a_script_reads_the_tables_back stdout ----
thread '…' panicked at factory/src/data.rs:886:9:
assertion `left == right` failed
  left: Nil
 right: Int(1)
```

落ちているのは

```ruby
$ore = recipe_of(:iron_plate)[:in]["iron_ore"]
```

の 1 行（`factory/src/data.rs:878`）で、**これがこの仕事で直したものそのもの**である。
map のキーが Symbol になったので `["iron_ore"]` は `nil` を返し、`[:iron_ore]` が 1 を返す。
games の側で rev を上げるときの読み替え（§5 の 3）は、**このテストが場所を教えてくれる**。
`a_machine_gives_one_picture_for_each_tile_it_covers` はまだ `line_of` を使っているので通る
（`take_with_lines` に替えたときに §5 の 2 の期待値が動く）。

これは**ソース互換ではあるが振る舞いは変わる**変更で、`expose` を使っている Ruby が既に
String でキーを引いていれば静かに `nil` になる、という実例でもある（§6 の 2）。

## 5. games の側で rev を上げたときにやること

この計画の外（sabiruby の main が push されたあと、rubevy_games の factory ブランチで）だが、
読み替えが要る場所を数えておく。

1. **`data::line_of` を消す**（`factory/src/data.rs:418` の 20 行と、`//!` の 39 行目の注意書き）。
   呼び出しは 6 か所（498・534・560・570・576・592 行）で、全部
   `Trouble { at: line_of(source, "machine", &name), … }` の形をしている。`take` を
   `take_with_lines` に替え、`Declared` の `line` をそのまま `at` に入れる。ソース文字列を
   引き回す引数（`source`）がそれで要らなくなるはず。
2. **期待値が 1 つ変わる。** `line_of` は「宣言が**始まる**行」、`Declared::line` は
   「**終わる**行」（= serde の raise と同じ）。`factory/src/data.rs:808` の
   `a_machine_gives_one_picture_for_each_tile_it_covers` の**後半**（`made_in: :smelter`、
   宣言されていない機械を指すレシピ）が `Some(4)` を期待していて、`GOOD` のそのレシピは
   4 行目と 5 行目に跨り `made_in:` は 5 行目にある。読み替えると **`Some(5)`** になり、
   同じ宣言を serde に断らせている `a_declaration_over_two_lines_is_reported_at_the_second`
   （`Some(5)`）と**同じ数になる**。**揃うことが直った証拠**なので、期待値と、その上の
   「Both are the declaration and neither is wrong, but they are not the same number」という
   コメント（4 行）を書き替える。前半（`size: [2, 2]`、3 行目の 1 行の `machine` 宣言）は
   `Some(3)` のまま。
3. **読み替える綴り。** `expose` が返す Hash の中の map のキーが Symbol になったので、
   `recipe_of(:iron_plate)[:in]["iron_ore"]` は **`[:in][:iron_ore]`** になる。
   実際に引いている所は**テストの 1 行だけ**で、rev を上げるとそこが落ちて場所を教えてくれる
   （§4.2 で実測: `factory/src/data.rs:878` の `$ore` の行、33 passed / 1 failed）。
   あわせて `factory/src/data.rs:651` の注意書き（「map のキーは String で返る」の段落まるごと）と、
   `//!` の 13・37 行あたりが `Vec<(String, T)>` と言っている所を直す。
   F3 が書くインサータの Ruby は最初から Symbol で引けばよい。
4. **`Options` を struct リテラルで作っている所は無い**（§2.3）ので、`#[non_exhaustive]` で
   直す所は無い。

## 6. 気づいた点

仕事の範囲の外で気づいたこと。直さずに書く（`implementer.md` の報告の形式 6）。

1. **`Vm::current_line` は名前が誘う。** `src/vm.rs:2621`。native の中から呼ぶと
   「呼び出し元の行」が返ると読めるが、返るのは**その次の命令の行**で、1 行ずれる
   （§1.2 の実測）。playground の stepper のためには正しい答えで、直すべきではない。
   今回 rustdoc に「これは次の命令の行、エラーの行は `backtrace_line`」と書き足し、
   `backtrace_line` を足して逃げ道は作ったが、**名前は変えていない**。`next_line()` の
   ような名前にするか（破壊的）、このままかは著者の判断。
   **属する先**: VM（sabiruby、`src/vm.rs`）。
2. **`expose` の答えの綴りが変わるのは、版を上げずに出せば黙った振る舞いの変更である。**
   `expose` が返す Hash の中の map のキーが String → Symbol になるので、既に
   `table[:field]["name"]` と引いている Ruby があれば `nil` になる。今その Ruby は
   **どこにも無い**（rubevy_games の Factory F2 は「答えること」しか確かめていない、
   §5 の 3）ので実害は無いが、`sabiruby-serde` は crates.io に 0.1.0 が出ている。
   **属する先**: 公開の判断（著者）。CHANGELOG の Unreleased に書いた。
3. **`tools/check_no_std.sh` が `sabiruby-serde` を見ないのは今回も効いた。**
   この crate の `no_std` は `docs/design/serde.md` の末尾に書いてある thumbv7em の
   直接ビルドを手で打つしかない。`host-scale-plan` の H6 がこれを直す段階として既にある。
   **属する先**: 計画済み（H6）。
4. **`docs/design/serde.md` に「fourteen cases」という数が埋まっていた**（`declare.rs` の
   テストの数）。テストを足すと嘘になる数で、実際に今回 19 になった。数を落として
   「`serde/tests/declare.rs`」だけにした。ほかにも散文に数が埋まっている所があるかもしれない。
   **属する先**: 文書の書き方（直した）。
5. **`serde/src/declare.rs` のコメントにあった `src/vm.rs:4264` が、この変更でずれた。**
   `Vm::native_call_args` は `backtrace_line` を足したぶん下にずれて 4293 になる
   （足す前から 2 行ずれてもいた）。行番号を落として名前だけにした。**他のファイルにも
   `src/vm.rs:NNNN` の形の参照がある**（`declare.rs` の rustdoc、design の文書）ので、
   VM に手を入れるたびに静かに嘘になる。**属する先**: 文書の書き方／VM。
6. **`cargo doc -p sabiruby-serde` が 1 件警告を出す**（`declare.rs:14` の
   `[`from_value`](crate::from_value)` が redundant explicit link）。この変更より前からある。
   **属する先**: バグというほどではない小物（sabiruby-serde）。
7. **`Declared<T>` に `#[non_exhaustive]` を付けたので、外の crate はパターンで
   分解するときに `..` が要る**（`let Declared { name, value, .. } = d;`）。
   フィールドを読むぶんには何も変わらない。games の側で書くときに一度だけ当たる。
   **属する先**: 使い勝手（報告のみ）。

## 7. 後日（同じ日）— `current_line` → `next_line`

§6 の 1 に対する著者の答え。**2026-09-21、著者「`Vm::current_line` は改名してよい」**、
新しい名前は `Vm::next_line`。作業場所は worktree `sabiruby-wt-next-line`
（ブランチ `next-line`、先頭は `574aeff`）。計画書の節は
[`serde-declare-lines-plan.md`](../plans/serde-declare-lines-plan.md) §4 に足した。

### 7.1 振る舞いは 1 バイトも変えていない

`src/vm.rs` の本体は `line_of(ci.pc)` のまま、名前だけ `next_line` にした。§1.2 で測った
「1 命令先を指す」はそのままで、それが playground のステップ実行の欲しい答えだからである。
rustdoc の 1 行目を「Source line of the instruction the innermost frame is at」から
**「Source line of the instruction that will run **next**」**に書き替えた。前の 1 行目は
返り値ではなく実装（`ci` と `pc`）を言っていて、名前と同じ誤読を誘っていた。

### 7.2 古い名前を消さずに残した — 理由は依存の形

`#[deprecated]` の別名 1 行（`self.next_line()`）にして残した。消さない理由は
計画書 §4 に書いたとおりで、**3 つの repo が 1 本の線で繋がっている**ことにある:

```
rubevy_games/Cargo.lock ──固定──> sabiruby の rev
        │                                 ▲
        │ pages.yml が建てる              │ path = "../../sabiruby"
        ▼                                 │
sabiruby-playground の main ──────────────┘
```

games の `pages.yml` は playground の main を、games の lock が固定している sabiruby の rev
（今は `50cae75`）に対して建てる。だから「sabiruby で古い名前を消す」と「playground が
新しい名前を使う」が同時に main に入ると、games の lock が上がるまでの間は Pages の CI が
必ず落ちる。別名が 1 つあれば、どの順で main に入ってもどちらの名前も通る。

**指示の前提を 1 つ直した。** 指示は「playground の CI は sabiruby の main を隣に
checkout する」と言っていたが、`sabiruby-playground/.github/workflows/pages.yml` を読むと
`SABIRUBY_REF`（**今は `7be7b86` = v0.5.2**）という**明示的に固定された commit**である。
つまり固定は 2 つあり、playground の側は自分の repo の中にも 1 つ持っている。
これは順番を 1 段増やす: playground が `next_line` を使う commit では、
**同じ commit で `SABIRUBY_REF` も上げないと playground 自身の Pages が落ちる**。
`7be7b86` は `backtrace_line` すら無い（v0.5.2）ので、別名があっても届かない。

別名を消す条件（games の lock が上がり、playground の main が `next_line` を使い、
`SABIRUBY_REF` もその rev にあり、両方の Pages が緑）と入れる順番は計画書 §4 に書いた。
**この仕事では消さない。**

### 7.3 別名が本当に警告を出すことを、テストにコンパイラに確かめさせた

`tests/native.rs` の `the_old_name_still_answers_the_same_and_says_it_is_deprecated`。
指示は `#[allow(deprecated)]` を付けた 1 本だったが、`#[allow]` では
**「警告が出ること」は確かめられない**（黙るだけで、`#[deprecated]` を外しても通る）。
`#[expect(deprecated)]` にした: `expect` はそれ自体が lint で、期待した警告が**出なければ**
`unfulfilled_lint_expectation` でコンパイラが文句を言う。つまりこのテストは

* `#[deprecated]` を誰かが外したら、
* 別名を消したら（`vm.current_line()` が無い）、

どちらでもコンパイルが止まる。1.85 の MSRV に対して `#[expect]` は 1.81 からなので使える
（`cargo check` を 1.85 で回す `msrv` ジョブを §7.5 で実際に通した）。中身は
`next_line` と `current_line` を同じ native の中で両方呼び、**同じ答えであること**と、
それが `[Some(2), Some(4)]`（呼び出しの 1 行先）であることを見ている。

### 7.4 repo の中に古い名前は残っていない

`grep -rn current_line --include='*.rs' --include='*.md' .` に残るのは 6 つのファイルだけで、
どれも「古い呼び出し」ではない（行番号は書かない。§6 の 5 で踏んだとおり、VM に手を入れる
たびに静かに嘘になる）:

| どこ | 何 |
|---|---|
| `src/vm.rs` | 別名の定義（`#[deprecated]`）、1 か所 |
| `tests/native.rs` | その別名のテスト（`#[expect(deprecated)]`）と、その rustdoc |
| `CHANGELOG.md` | Unreleased の、改名そのものの記述 |
| `docs/design/serde.md` ・ `docs/design/inspect.md` | 「前の名前は `current_line`」の括弧 1 つずつ |
| `docs/plans/serde-declare-lines-plan.md` | §2 の本文（当時の記録、触らない）と §4（改名の節） |
| `docs/worklog/2026-09-21-serde-lines.md` | §1・§3・§6（当時の記録、触らない）とこの節 |

**過去の worklog と、済んだ計画書の本文は書き換えていない** — その時点で正しかった記録で、
今の名前に直すと「なぜ改名したか」が読めなくなる。書き替えたのは「今」を言う文書だけ
（`docs/design/*`、`docs/README.md`、`CHANGELOG.md`）で、design の 2 か所には
「（当時 `current_line`）」と旧名を 1 語だけ残した。設計文書を読んで `git log` を引く人が
橋を渡れるようにするためで、別名を消すときにこの 2 語も消える。

### 7.5 確かめたこと

```
$ cargo test --workspace                       TOTAL passed: 251  failed: 0   （250 → +1）
$ ./tools/check_no_std.sh                      no_std OK
$ cargo build --workspace --all-targets        警告 0（deprecation も 0）
$ git diff main -- '*.rs' | grep '^+' | grep -c unsafe     0
$ ./target/release/sabiruby mrbtest tests/mrbtest/assert.mrb …（112 ファイル）
$ diff tests/mrbtest/baseline.txt <(…)         BASELINE IDENTICAL
```

本家テストは前任と同じ道（Docker の `tools/mrbtest.sh` ではなく、チェックインされている
`.mrb` を release の CLI で回して baseline と比べる）。VM の計算は変えていないので 1 度。

### 7.6 翌日（2026-09-22）— 別名を消した

**著者「`Vm::current_line` の別名を今 main で消す」。** 作業場所は worktree
`sabiruby-wt-drop-alias`（ブランチ `drop-alias`、先頭は main の `8d0fea2`）。
§7.2 で数えた 3 つのピンはどれも動き終えていて、着手前に自分でも確かめた:

* rubevy_games `6ea7c68` の `Cargo.lock:4398` が
  `git+https://github.com/sabiruby/sabiruby#8d0fea2993e75d05999c0a223b610123beeed86a`。
* sabiruby-playground の main `f639f58` は `wasm/src/lib.rs:316,324` で `next_line` を呼び、
  `.github/workflows/pages.yml:21` の `SABIRUBY_REF` も同じ `8d0fea2…`。
* 本体の報告で、どちらの Pages も緑、公開ページに対する `test/browser.mjs` も全通過。

消す前に、外に呼び出しが 1 つも残っていないことを先に見た（1 件でもあれば消さずに止まる、
という順番で）。`rubevy` `rubevy_games` `sabiruby-playground` `mruby-porting-kit` `sabiruby`
の `*.rs`（`target/`・`node_modules/`・過去の worklog を除く）で:

```
$ grep -rn current_line --include='*.rs' {rubevy,rubevy_games,sabiruby-playground,mruby-porting-kit,sabiruby}
sabiruby/tests/native.rs:280  （別名のテストの rustdoc）
sabiruby/tests/native.rs:295  （そのテストの中）
sabiruby/src/vm.rs:2633       （別名の定義）
```

この repo の中の 3 行だけで、外は 0 件。同じ機械に並んでいる 10 個の worktree
（`rubevy-wt-*`、`rubevy_games-wt-*`、`sabiruby-playground-wt-next-line`）も同じ grep で
0 件だったので、別のブランチで作業している担当の足も踏まない。

消したのは `src/vm.rs` の `#[deprecated] pub fn current_line`（4 行）と、`tests/native.rs` の
`the_old_name_still_answers_the_same_and_says_it_is_deprecated`。**§7.3 の仕掛けがこの日
仕事をした**: `#[expect(deprecated)]` は別名が無くなればコンパイルが止まるので、
「別名を消したのにテストだけ残って緑」という状態にはそもそもなれない。
`a_native_asks_backtrace_line_for_the_line_it_was_called_from`（`next_line` と
`backtrace_line` が別の問いであることを見るほう）はそのまま残っている — 改名はそのために
したのだから、こちらが本体である。

確かめたこと（`5b4c0a5` の時点。数はすべて実行した出力から）:

```
$ cargo test --workspace         着手前 TOTAL passed: 251  failed: 0
                                 消した後 TOTAL passed: 250  failed: 0   （別名のテスト 1 本ぶん）
$ ./tools/check_no_std.sh                                   no_std OK
$ cargo build --workspace --all-targets                     警告 0（5 crate を建て直して 1 行も出ない）
$ RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --workspace --lib
                                 Finished — 消した項目への intra-doc リンクは残っていない
$ cargo +1.85 check --workspace --all-targets               Finished（MSRV）
$ git diff main -- '*.rs' | grep '^+' | grep -c unsafe      0
```

利用者の側も、どちらの repo にも書かずにスクラッチパッドで建てた。playground は
§7.2 の図のとおり `path = "../../sabiruby"` で隣を見るので、隣にこのブランチを置けばよい:
`pg/sabiruby` をこの worktree への symlink、`pg/sabiruby-playground` を main の
`git archive` から展開したコピーにして、`wasm/` で `cargo check`。`wasm/.cargo/config.toml`
が `target = "wasm32-wasip1"` を立てているので C の側に wasi-sdk が要り、素で回すと
`cc-rs: failed to find tool "clang"` で落ちる。`tools/build.sh` が探すのと同じ
`~/.local/wasi-sdk-34.0-x86_64-linux` を `CC_wasm32_wasip1` / `AR_wasm32_wasip1` に渡すと
`Finished`（警告なし）。rubevy_games も同じくコピーの側だけを触り、根の
`[patch.crates-io]` の 3 行を git からこの worktree の path に書き替えて
`cargo check --workspace --all-targets`（`CARGO_TARGET_DIR` もスクラッチパッド）。

**版**: 公開のメソッドを消すのは破壊的変更なので、次に公開する `sabiruby` は
**0.6.0**（§7.5 の時点の見込みだった 0.5.3 ではない）。ここでは上げない（公開は著者）。
上げるときに**一緒に**直さないと止まるところが 6 つある。どれも `sabiruby` への
version 要求で、caret なので `0.5.x` の要求は `0.6.0` を受けない:

| どこ | 今 |
|---|---|
| `serde/Cargo.toml` | `sabiruby = { path = "..", version = "0.5", … }` |
| `cli/Cargo.toml` | `sabiruby = { version = "0.5.0", path = "..", … }` |
| `compiler/Cargo.toml` | `sabiruby = { version = "0.5.0", path = "..", … }` |
| rubevy `Cargo.toml`（2 か所） | `sabiruby = "0.5.1"` と `{ version = "0.5.1", features = ["macros"] }` |
| rubevy_games `Cargo.toml` | `sabiruby = "0.5.1"` |

外の 2 つは crates.io の話だけではない。games は `[patch.crates-io]` で全部を git に送って
いて、**patch した crate の版が要求を満たさないと cargo は patch を使わない**（crates.io の
0.5.x に落ちるか、2 つ目の VM が入る）。つまり `Cargo.toml` の `version` を 0.6.0 にした
その日から、rubevy と rubevy_games の要求を上げるまで games は今までどおりには建たない。
rubevy は games の git 依存なので、rubevy を先に直して push する順番になる。
版を上げるかどうかと順番は著者が決めることなので、ここでは触っていない。

**気づいた点**（7.6 の範囲で）:

1. **`docs/README.md` の目次が、旧名を橋として使っていた。** 「`Vm::next_line`（当時
   `current_line`）」という括弧は、この worklog が全編 `current_line` で書かれていることの
   断り書きでもあった。括弧は外したが、代わりに「なぜこの記録は旧名で書かれているか」を
   最後の 1 文（改名の節への案内）で言うようにした。旧名を消すときに、**旧名で書かれた
   記録への入口まで消さない**、が次に同じことをするときの教訓。
   **属する先**: docs の書き方（報告のみ）。
2. **playground の `cargo check` は wasi-sdk が要る。** `wasm/.cargo/config.toml` が
   target を立てているので、「素の `cargo check` で見られる」と思って回すと C の
   ツールチェーンで落ちる。`README` の Building には書いてあるが、**検証の手順としては
   `tools/build.sh` の中にしか環境変数の組み立てが無い**ので、毎回読み直すことになる。
   **属する先**: sabiruby-playground の使い勝手（報告のみ、直していない）。

## 8. main の CI が赤かった件 — `cargo doc` の警告 1 件

改名とは別の話だが同じブランチで直した（別コミット）。**main の CI が 09-20 の declare の
マージ（`944b72b`）から 3 回続けて赤く**、落ちていたのは `test` ジョブの
`cargo doc --no-deps --workspace --lib`（`.github/workflows/ci.yml:30`、`RUSTDOCFLAGS: -D warnings`）。

§6 の 6 が「この変更より前からある警告 1 件」と書いて通り過ぎたもので、**CI は
`-D warnings` でそれをエラーにしている**。ローカルで同じ環境変数を付けて再現した:

```
$ RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --workspace --lib
error: redundant explicit link target
  --> serde/src/declare.rs:14:20
   |
14 | //! [`from_value`](crate::from_value) reads as a `T` …
   |      ----------    ^^^^^^^^^^^^^^^^^ explicit target is redundant
error: could not document `sabiruby-serde`
```

`declare.rs:83` が `use crate::{from_value, …}` しているので `from_value` は
このモジュールのスコープで既に解決する。明示の target を落として `[`from_value`]` だけにした。
rustdoc が出していた suggestion そのままで、リンク先は変わらない。

改名で足した rustdoc のリンク（`[`Vm::next_line`]` を 2 か所、`[`Vm::backtrace_line`]`）も
同じコマンドで確かめてある。CI の step のうちローカルで回せるものを一通り:

```
$ cargo test --workspace                                        251 passed / 0 failed
$ cargo test -p sabiruby --no-default-features --features "std utf8"   全部 ok
$ tools/check_no_std.sh --features regexp                       no_std OK
$ tools/check_no_std.sh --features utf8                         no_std OK
$ cargo build -p sabiruby --lib --target wasm32-unknown-unknown  Finished
$ RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --workspace --lib      Finished（0 警告）
$ RUSTDOCFLAGS='-D warnings' CARGO_TARGET_DIR=target/doc-cli \
      cargo doc --no-deps -p sabiruby-cli --bins                Finished
$ cargo publish --dry-run --workspace                           5 crate とも Finished、exit 0
$ cargo +1.85 check --workspace --all-targets                   Finished（msrv ジョブ）
```

**全部通る。赤くなる理由はこの 1 件だけだった。** `--all-targets` の 1.85 が通るので、
§7.3 の `#[expect(deprecated)]`（1.81 から）も MSRV の内側である。`publish --dry-run` は
作業木が汚れていると止まる（`error: 1 files in the working directory contain changes that
were not yet committed`）ので、コミットしてから回した。

ci.yml に clippy と fmt の step は無い（この repo は `cargo fmt` を使わない方針で、
`rustfmt.toml` も無い）ので、回すものは上で全部である。別ジョブの `msrv` は
`dtolnay/rust-toolchain@1.85` で `cargo check --workspace --all-targets` だけで、
ローカルでは `rustup` に入っている 1.85 を `CARGO_TARGET_DIR` を別にして回した
（共有の target に 1.85 の成果物を混ぜないため）。

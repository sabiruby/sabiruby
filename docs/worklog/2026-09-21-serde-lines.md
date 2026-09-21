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

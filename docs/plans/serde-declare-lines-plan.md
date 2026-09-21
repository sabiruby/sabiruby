# sabiruby-serde: 宣言の表が行を覚える／map のキーを Symbol で返す — 実装指示書

作成 2026-09-21。出どころは rubevy_games の 3 本目 Factory の段階 F2（data stage）。担当が「VM（sabiruby-serde）の話」として挙げた 2 件で、
著者が同日「両方 F3 の前に直す」と決めた。記録は rubevy_games `docs/worklog/2026-09-21-factory-F2.md`（§3.2・§9 の 1 と 2）と
`docs/plans/factory-plan.md` §7 の 09-21 F2 の最初の 2 行。利用者の側の実物は rubevy_games `factory/src/data.rs`
（`line_of` という迂回と、651 行あたりの「map のキーは String で返る」という注意書き）と `factory/ruby/data.rb`。
対象: sabiruby main `50cae75`。行番号はこの時点のもの。着手前に実物と合っているか確かめる。

---

## 0. はじめの一歩

```bash
cd /home/kishima/book/kishima
git -C sabiruby worktree add ../sabiruby-wt-serde-lines -b serde-lines main
cd sabiruby-wt-serde-lines && cargo test --workspace && ./tools/check_no_std.sh
```

作法は `/home/kishima/book/.claude/agents/implementer.md`（「Rust の VM（sabiruby）に関して」の節）と `/home/kishima/book/CLAUDE.md`。
**VM の crate は `no_std + alloc`、unsafe は禁止（serde の crate にも足さない）、`Vm` は `Send + Sync`、本家テストの基準を下回らない。**
段階ごとに 1 コミット、push しない、main に触らない、過程は `docs/worklog/2026-09-21-serde-lines.md` に。`cargo fmt` を走らせない。`git stash` を使わない。
`tools/check_no_std.sh` は `sabiruby-serde` を見ない（host-scale-plan の H6）ので、serde の crate は
`cargo build -p sabiruby-serde --no-default-features`（か、その crate の no_std の確かめ方。無ければ worklog に「確かめる道が無い」と書く）で見る。

## 1. 何を直すのか

### D1. `Options::symbol_keys` が map のキーに届かない

`serde/src/ser.rs` の `Serializer::key`（36 行）は struct のフィールド名と variant 名にしか通っていない。`MapSer::serialize_key`（225 行）は
キーを普通の値として書くので、`BTreeMap<String, u32>` は `Options::symbol_keys()` の下でも `{"iron_ore" => 1}` になる。
`declare::expose` は `Options::symbol_keys()` で表を返す（`serde/src/declare.rs:312`）ので、Ruby で

```ruby
recipe :iron_plate, in: { iron_ore: 1 }, out: { iron_plate: 1 }, time: 2.0, made_in: :furnace
```

と Symbol で書いた名前が、読み返すと `recipe_of(:iron_plate)[:in]["iron_ore"]` と String になる。**書いた綴りと読む綴りが違う。**
F3 でインサータの Ruby がこれを読み、そこから先は利用者のスクリプトに出る綴りなので、その前に直す。

**設計（本体が原則から決めた。違和感があれば止めて報告）:**

- `symbol_keys` の意味は**変えない**。既存の利用者（`Options::symbol_keys()` を呼んでいる人）の map が黙って Symbol になるのは、
  見えない所で振る舞いが変わる変更である。それに、フィールド名は有限（型が決める）だが **map のキーは実行時の値で、数に上限が無い**。
  Symbol の表が回収されないなら（**着手前に `Vm::intern` の表が GC されるかを確かめて worklog に書く**）、任意の map のキーを
  intern するのは利用者が選ぶべきことで、フィールド名と同じスイッチに相乗りさせない。
- `Options` にフィールドを 1 つ足す: **`symbol_map_keys: bool`**（既定 `false`）。map のキーが**文字列として serialize されるとき**だけ
  Symbol にする（整数のキーなどはそのまま）。コンストラクタは `Options::symbols()`（両方 `true`）を足し、`Options::symbol_keys()` は今のまま。
  `Options` は公開フィールドの struct で `#[non_exhaustive]` ではないので、フィールドを足すのは struct リテラルで作っている外の利用者には
  破壊的変更である。**外の利用者は今いない**（rubevy も games も `Options::symbol_keys()` か `default()` しか使っていないことを grep で確かめる）。
  この機会に `#[non_exhaustive]` を付け、作り方をコンストラクタと `..Default::default()` に寄せる。
- `declare::expose` / `expose_on` は `Options::symbols()` で返す。宣言の表の map のキーは**宣言の名前**（アイテム名など）で、
  宣言の数しか無く、Ruby の側で Symbol で書かれたものだから。rustdoc（`declare.rs:252` あたり）にそう書く。
- 読む側（`de.rs`）は既に String と Symbol の両方を受ける（`lib.rs:39` の表）。**往復のテスト**を足す:
  Symbol のキーで書いた Hash → `T` → `Options::symbols()` で書き戻し → 元の Hash と `==`。
- `docs/design/serde.md` の対応表と、`lib.rs` の頭の表を直す。

### D2. `Declarations::take` の表が行を覚えていない

`Declarations::take` は `Vec<(String, T)>` を返す。宣言**の中**の誤り（未知のフィールド、型）は native の中で raise するので backtrace が行を持つが、
宣言を**またぐ**検査（「このレシピが言うアイテムは宣言されていない」）はホストが `take` の後でやるしかなく、そのとき**どの行の宣言だったかが無い**。
games は `data.rb` のソースを自分で引いて `recipe :name` の行を探している（`data::line_of`、約 10 行）。害は無いが、
「宣言が始まる行」を返すので、serde の誤りが言う行（呼び出しの行 = backtrace の先頭）と**同じ宣言で違う数になりうる**（2 行にまたがる宣言で実測）。

**設計:**

- 宣言の native（`define_on` のクロージャ、`declare.rs:165` あたり）が、呼ばれた時点の **`Vm::current_line()`**（`src/vm.rs:2621`）を控える。
  これが backtrace の先頭フレームと**同じ出どころの同じ数**であることをテストで示す（2 行にまたがる宣言で、わざと型を間違えた版が raise で言う行と、
  正しい版が控えた行が一致する）。`current_line()` が native の中から呼び出し元の Ruby の行を返さないなら（native のフレームを指すなど）、
  **VM の側に safe な口を足してよい**（unsafe は禁止。足したら報告に明記）。足さずに済む道を先に探す。
- 公開の形: `take` の返り値の型は変えない（今の利用者を壊さない）。**`take_with_lines(self, vm) -> Vec<Declared<T>>`** を足す。
  `pub struct Declared<T> { pub name: String, pub value: T, pub line: Option<u32> }`（`#[non_exhaustive]`）。
  `line` が `Option` なのは、デバッグ情報の無い `.mrb` から読まれた宣言には行が無いから（そのときの `current_line()` の値を確かめて合わせる）。
  `take` は `take_with_lines` の上に書く（控える場所は 1 つ）。`define_replacing` で上書きされた宣言は**後の宣言の行**を持つ。
- ファイル名は持たない。`current_line()` と同じ粒度に留める（games はブラウザでファイル名を自分で付け直している。ページのコンパイラが全部を
  `playground.rb` と呼ぶため — それは playground と games の側の話で、ここでは広げない）。
- `docs/design/serde.md` の declare の節に足す。

## 2. 段階

| 段階 | 到達点 | 確認 |
|---|---|---|
| **D1** | `Options::symbol_map_keys`・`Options::symbols()`・`#[non_exhaustive]`、`expose` が Symbol のキーで返す | 往復のテスト。`symbol_keys()` だけの下では map のキーが String のまま（前の振る舞いが変わっていない）テスト。`cargo test --workspace`、serde の no_std の確認 |
| **D2** | `Declared<T>`・`take_with_lines`、行は backtrace の先頭と同じ数 | 2 行にまたがる宣言のテスト、上書きのテスト、行の無い場合。`cargo test --workspace` |
| **D3** | docs（`design/serde.md`、`docs/README.md` の目次に計画と worklog、この計画書の状況）。版を上げるかは**上げずに報告**（公開は著者） | — |

games の側（rev を上げ、`data::line_of` を消し、`recipe_of(:x)[:in][:iron_ore]` に読み替える）はこの計画の外。sabiruby の main が push された後に
rubevy_games の factory ブランチでやる。**rubevy と rubevy_games が今の main でそのまま建つこと**（API を壊していないこと）は D3 の最後に
`[patch]` をローカルの worktree に向けた一時的なビルドで確かめ、確かめたら戻す（コミットしない）。

## 3. 状況

| 段階 | 状況 |
|---|---|
| D1〜D3 | 未着手（2026-09-21 作成） |

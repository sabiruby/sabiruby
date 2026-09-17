# 2026-09-18 エディタの色付けのための `highlight()`（段階 H0）

`rubevy_games/docs/plans/editor-highlight-plan.md` の段階 H0。箱庭のエディタに構文色付けを
入れるために、Ruby のソースを 9 種に分類する口を `sabiruby-compiler` に足す。分類は自前で
書かず、すでに vendor にある Prism の字句解析に決めさせる。移す元は family-mruby の
`fmruby-core/lib/add/picoruby-syntax-highlight/src/syntax_highlight.c`（325 行）。

## まず、移す元を読む

`syntax_highlight.c` は 3 つの部品でできていた。

1. `token_type_to_category(pm_token_type_t)` — Prism のトークン型を 0〜8 に落とす switch。
   ここが分類の本体で、134 行ある。
2. `highlight_callback(void *data, pm_parser_t *parser, pm_token_t *token)` — Prism の
   `pm_lex_callback_t` に刺すコールバック。トークンが 1 つ切り出されるたびに呼ばれ、
   `token->start - parser->start` から `token->end - parser->start` までのバイトに分類を
   書く。`HIGHLIGHT_DEFAULT` のときは何も書かずに戻る（表は最初に `memset` で 0 にして
   あるので同じこと）。
3. `highlight_visit_node(const pm_node_t *node, void *data)` — **構文木をもう一度歩く
   第 2 段**。`pm_visit_node` で木を回り、`PM_CALL_NODE` の `message_loc`、`PM_DEF_NODE` の
   `name_loc` を分類 8（メソッド名）で、`PM_SYMBOL_NODE` の開き・値・閉じをまとめて分類 5
   で塗り直す。

そして `fmrb_syntax_highlight_line()` が `pm_parser_init` → `lex_callback` を設定 →
`pm_parse` → `pm_visit_node` → `pm_node_destroy` → `pm_parser_free` の順に呼び、
`mrb_syntax_highlight_tokenize()` が mruby の `String` に包んで返す。picoruby の値の型に
関わるのは最後の 1 つだけなので、そこは捨てる。

**ここで計画書と食い違いが 1 つ見つかった。** 計画書 §5 の 3 は「family-mruby は『直前の
トークンが `def` か `.`』で判定している（字句コールバックの中で前のトークンを覚える）」と
書いているが、実際の family-mruby はそうしていない。上の 3 の AST パスで木から読んでいる。
依頼のプロンプトも §5 の 3 を指して「直前のトークンで判定」と指示しており、計画書 §3.1 の
手順（`pm_parser_init` → `lex_callback` → `pm_parse` → `pm_node_destroy` →
`pm_parser_free`）にも `pm_visit_node` が入っていない。つまり計画としては「AST パスを捨て、
字句だけでやる」で一貫している。指示どおり字句だけで実装し、木を歩かないことで何が変わるか
を測って下に書いた。捨てた案は「family-mruby と同じく `pm_visit_node` も移す」で、
捨てた理由は 2 つ。指示がそう言っていないこと、そして木を歩かない方が**構文エラーのある
入力に強い**こと（後述）。

## Prism の版

計画書 §5 の 1 が警戒している「版のずれでトークン名が変わっている」は起きなかった。
`compiler/vendor/prism/include/prism/version.h` は `PRISM_VERSION "1.9.0"`、family-mruby 側
（`fmruby-core/vendor/ti_prism`、`fmruby-core/vendor/spinel/vendor/prism` など）も 1.9.0。
念のため機械で照合した。`vendor/prism/generated/include/prism/ast.h` の `pm_token_type_t` から
`PM_TOKEN_*` を 165 個抜き出し、`syntax_highlight.c` が名前で参照している 70 個ほどと
`comm -13` で突き合わせたところ、**こちらに無い名前は 0 個**。switch はそのまま移せた。

## 分類 8（メソッド名）を字句だけで出す

木を歩かないので、`def foo` の `foo` や `a.b` の `b` を塗る手が要る。指示どおり、
コールバックに「直前のトークン」を 1 つ持たせた。

```c
typedef struct {
  uint8_t         *map;
  size_t           size;
  pm_token_type_t  prev; /* last token that was not a newline or a comment */
} highlight_data_t;
```

`prev` を更新するときに `PM_TOKEN_NEWLINE` / `PM_TOKEN_IGNORED_NEWLINE` / `PM_TOKEN_COMMENT`
を飛ばしているのは、次のような書き方が箱庭の脚本にあるため。

```ruby
obj
  .foo
  # ここで一言
  .bar
```

改行とコメントを飛ばさないと `foo` と `bar` の直前が NEWLINE / COMMENT になって 8 にならない。
飛ばす実装で測ったら `00000008880003333000888` になった（`foo` も `bar` も 888）。

`&.` も `.` と同じ扱いにした。`a&.b` は `a.b` と同じ呼び出しで、読む人にとっても同じ「`.` の
後ろ」だから。これは指示の文面（「`def` か `.`」）からの小さな踏み込みなので、ここに書いて
おく。テストにも 1 行（`assert_eq!(map("a&.b"), "0008")`）入れた。

### 木を歩かないと何が変わるか（実測）

| 入力 | 字句だけ（今回） | family-mruby（AST パスあり）なら |
|---|---|---|
| `a.b(1).c` | `00804008` | 同じ |
| `p 1` | `004` | `p` が 8（`PM_CALL_NODE` で VARIABLE_CALL ではない） |
| `def ==(o)` | `1110000000…`（`==` は 0） | `==` が 8（`name_loc`） |
| `def foo=(v)` | `111088880000…`（`foo=` は 1 トークン） | 同じ |
| `:sym` | `5000`（`:` だけ 5） | `5555`（`PM_SYMBOL_NODE` が全体を塗る） |
| `:Plant` | `566666`（`:` が 5、`Plant` が 6） | `555555` |

**塗り足りないことはあっても、塗り間違えることはない**（AST パスが塗る範囲の部分集合に
なっている）。`p 1` と `def ==` は色が付かないだけ、`:sym` は `:` に色が付いて名前は既定色。
`:Plant` が定数色になるのは計画書 §1 の表の例（分類 5 の例として `:Plant`）と食い違うので、
報告に上げた。直すなら `prev == PM_TOKEN_SYMBOL_BEGIN` のときも 5 にする 1 行で、
メソッド名と同じ仕組みで済む。今回は計画書 §3.1 が分類 5 に挙げているトークンが
`SYMBOL_BEGIN` と `LABEL` の 2 つだけなので、勝手に増やさずそのままにした。

## 上限を置かなかった

family-mruby は `HIGHLIGHT_MAX_SOURCE_SIZE 32768` を持っていて、それを超えると `nil` を返す。
これを写さず、まず**実際の入力を測った**（2026-09-18 の `rubevy_games/garden/ruby/`）。

| ファイル | バイト |
|---|---:|
| `prelude.rb` | 21433 |
| `world_prelude.rb` | 17161 |
| `world.rb` | 16249 |
| `creatures/beetle.rb` | 5870 |
| `creatures/rabbit.rb` | 3632 |
| 合計 | **64345** |

つまり family-mruby の 32 KiB は、この箱庭のソース全体（62.8 KiB）を**そのまま撥ねる**数
だった。あの 32 KiB は ESP32 の固定ヒープ（数百 KB）から来ている数で、こちら（std のホスト、
デスクトップとブラウザ）には対応する予算が無い。

そしてエディタが一度に持つのは 1 ファイルで、計画書の既定 7 が言うとおり prelude は映らない
（解析も本文だけ）。いちばん大きい単独のバッファは `world.rb` の 16249 バイト。表の費用は
ソース 1 バイトにつき 1 バイトで、そのソースは呼ぶ側がすでに持っている。

時間も測った（release、best of 20、この機械）。

| 入力 | バイト | best |
|---|---:|---:|
| `prelude.rb` | 21433 | 198.8 µs |
| `world_prelude.rb` | 17161 | 91.8 µs |
| `world.rb` | 16249 | 136.3 µs |
| `creatures/beetle.rb` | 5870 | 41.5 µs |
| `creatures/rabbit.rb` | 3632 | 59.3 µs |
| 5 つを連結 | 64345 | 580.3 µs |

計画書の既定 5（毎フレームではなく本文が変わったときだけ解析する）を守れば、16 KB で
136 µs は 60 fps の 1 フレーム（16.7 ms）の 0.8% で、打鍵のたびに払っても見えない。

**結論: 上限は置かない。** 導ける予算が無く、測った実データはどの候補値よりも小さく、
置けば「根拠のない数」になる。`highlight()` は必ず `src.len()` の表を返す、という 1 つの
約束だけにした。`csrc/shim.c` の頭にこの数字と理由を書いてある。

（副作用として API が単純になった。上限があれば「表を返せない」場合を
`Option<Vec<u8>>` か空 `Vec` で表さねばならず、計画書 §3.1 の
`pub fn highlight(src: &[u8]) -> Vec<u8>` という型にはその席が無い。）

## unsafe とロック

`compiler/src/ffi.rs` の既存の `unsafe extern "C"` ブロックに宣言を 1 つ足しただけで、
`unsafe extern "C"` ブロックは増えていない。呼ぶ側の `unsafe { … }` は 1 つ増える（extern
関数の呼び出しは unsafe でしか書けない）。crate の中で unsafe が住んでいるファイルは
`ffi.rs` のままで、SAFETY コメントは既存の `compile` と同じ形にした。

`lib.rs` の `compile` は `mrc_presym.c` の静的変数のためにグローバルロックを取る。
`highlight` は取らない。`sabiruby_mrc_highlight` が呼ぶのは Prism だけ
（`pm_parser_init` / `pm_parse` / `pm_node_destroy` / `pm_parser_free`）で、
`vendor/prism` の中に `mrc_presym.c` を参照するファイルは無く（`grep -rl presym` の結果は
`vendor/mruby-compiler/src/` の 4 ファイルだけ）、`vendor/prism/src/**` に可変のファイル
スコープ変数も無い。これで、別のスレッドがコンパイルしている間もエディタは色を塗れる。
`ast()` は同じく Prism しか呼ばないのにロックを取っているが、そちらは触っていない。

## 構文エラーのある入力

エディタの本文は、見られている時間のほとんどで壊れている。木を歩かないことがここで効く。
`pm_parse` は回復するので木は返るが、字句コールバックは**解析が失敗するより前に、
読めたところまで**を全部書き終えている。

```
"def foo(\n  @a = :x\n" => "1110888000077000500"
```

`def` が 111、`foo` が 888、`@a` が 77、`:` が 5。閉じ括弧が無くても表は返る。
`"end end end \"unterminated"` は `"1110111011102222222222222"`（閉じていない文字列が
最後まで 2）。

## テストの期待値は推測しない

`compiler/tests/highlight.rs`。期待値は 1 つも推測で書いていない。実装を入れたあと、
`compiler/examples/hlprobe.rs` という使い捨ての例を置いて、テストに使う入力そのものを
食わせ、返ってきた表を 1 バイト 1 桁の文字列として印字させ、**その出力をテストに貼った**
（例はコミット前に消した）。測って初めて分かったのは次の 5 つ。

* **式展開**: `x = "a #{b} c"` → `00002222202222`。`#{` と `}` が 2 で、中の `b` は 0。
  計画書 §5 の 2 の予想どおりだったが、確かめてから書いた。
* **正規表現**: `x =~ /re/` → `000002222`。`/re/` は 4 バイトとも 2。`PM_TOKEN_REGEXP_BEGIN`
  と `PM_TOKEN_REGEXP_END` を分類 2 にした結果で、中身は `STRING_CONTENT` なのでやはり 2。
* **ラベル**: `f(key: 1, :sym => 2)` → `00555504005000000040`。`key:` は `PM_TOKEN_LABEL`
  1 つで、コロン込みで 4 バイトとも 5。いっぽう `:sym` は `:` だけが 5。
* **日本語のコメント**: `# 甲虫は歩く` は 17 バイトで、17 バイトとも 3。**そして
  コメントを終える改行もコメントのトークンに入っている**（`"def leaf\n  @n = 1 # ha\nend\n"`
  → `"111088880007700040333331110"` の 22 番目が 3）。これは読む前は知らなかった。
  分類が変わるバイト位置が全部 `is_char_boundary` であることもテストで確かめている
  （計画書 §5 の 4。egui の `append` は `&str` を取るので、文字の途中で切ると落ちる）。
* **ヒアドキュメント**: `s = <<~TXT\n  hi\nTXT\n` → `00002222220222222222`。`<<~TXT` を
  終える改行だけが 0 で、本文も終端子 `TXT` も、その後ろの改行も 2。

テストは 11 本、全部通る。

## 版と文書

計画書 §3.1 の「版は `0.5.2`（`Cargo.toml`）」を、リポジトリの release の名前と読んで
ルートの `sabiruby` を 0.5.1 → 0.5.2 にした。中身が変わったのは `sabiruby-compiler` の
ほうなので、そちらも 0.2.2 → 0.2.3 に上げている（新しい公開関数が 1 つ増えたので、
この版を publish しないと H1 以降が使えない）。CHANGELOG はこの 2 つを分けて書いた。
`sabiruby` 側が中身の変更なしに版だけ動くのは、0.5.0 で `sabiruby-compiler` 0.2.2 が
同じことをした前例がある。ここは著者の判断があれば戻せる（報告に上げた）。

`docs/design/compiler.md` の「Layout」と「The shim」、`compiler/README.md` の API、
CHANGELOG、この worklog と `docs/README.md` の目次。

## 確認

* `cargo test`（workspace 全体）
* `cargo test -p sabiruby-compiler`（golden 含む。Docker の要る `tools/mrbtest.sh` ではなく
  `target/release/sabiruby mrbtest` で `tests/mrbtest/*.mrb` を回して baseline と比較）
* `tools/check_no_std.sh`
* `cargo clippy --workspace --all-targets` を着手前（main `8faf26f`）と後で数える
* `tools/bench.sh` は取らない。VM に 1 行も触っていない（変更は `compiler/` と文書だけ）。

結果は報告と下の「結果」に。

## 結果

（実行した出力の要点は報告に貼った。ここには数だけ残す。）

* `cargo test --workspace`: 失敗 0。新しい `compiler/tests/highlight.rs` は 11 本すべて通過。
* `tools/check_no_std.sh`: 通過（VM crate は触っていない）。
* mrbtest: baseline と同じ。
* clippy: 着手前と同数。

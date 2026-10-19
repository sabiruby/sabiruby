# mruby-regexp のエンジンを本家のものに置き換える計画

作成 2026-10-19（案）。著者「正規表現の後読みと後方参照はどうすれば対応できる？」「それぞれ計画書書いてみて」（2026-10-19）。
調査は本体が依頼したもの（2026-10-19、読むだけ）。行番号は 4.1.0-rc2 の `mrbgems/mruby-regexp/` から。

## 目的

今の mruby-regexp は照合を `regex-automata`（有限オートマトン）に任せている。そのため、次の構文を `RegexpError` で断っている（`docs/design/gems.md` "Deviations kept"）。

- 後読み・先読み
- 後方参照と `\k`
- `\g<…>`
- アトミックグループ
- 強欲な量指定子
- 条件分岐
- 不在演算子

両方でコンパイルできるのに答えが違うものも 39 assertion ある。`\Z`、末尾の改行の後の `^`、空の反復の捕獲、`/i` の畳み方、上限、バイトとして読む subject、文言である。

本家 4.1.0-rc2 の mruby-regexp は自前のエンジンを持つ。これを Rust に写して置き換え、互換の方針（本家に合わせる）どおりに、書ける構文・答え・文言・上限を本家と同じにする。

- **今の失敗の数**: エンジンが原因で落ちている本家のテストは、utf8 で 124（KO 41、crash 83）、bytes で 101。no-regexp では 0。
- **目標**: このうち、本家の `mruby` コマンドも同じ答えを出すものを全部通す。

## 本家のエンジンの作り（調査の要約）

- **照合の器**: 3 つあり、コンパイル時にパターンごとに 1 つを選ぶ（`src/re_exec.c:1945-1958`）。
  - `literal_exec`: 文字だけのパターン。
  - `pike_vm`: Thompson/Pike の NFA。線形時間で、段数の上限が無い。
  - `backtrack_exec`: 後方参照・先読み・後読み・アトミック・`\g`・不在・条件分岐と、一部の最短一致を受け持つ。上限は段数とスタック。
- **コンパイラ**: 構文木を作らず、1 パスで 4 バイトの命令（約 40 種）を出す（`re_compile.c`）。入れ子はヒープのスタックで扱い、C の再帰は文字クラスの入れ子（256 段まで）だけ。
- **ロジックの行数**:
  - `re_compile.c` が code 約 3,100 行
  - `re_exec.c` が code 約 1,240 行（うち pike が 408、backtrack が約 550）
  - `re_internal.h` が約 330 行
- **表**（どれも Unicode 17.0.0 からの生成物）:
  - `re_ctype.h`（約 3,480 項目）
  - `re_cased.h`（32 範囲）
  - コアの `src/unicase.c`／`unicase.h`（`/i` の fold と unfold）
- **VM との境目**: 例外（コンパイルエラーだけ）、確保、コアの文字の復号と大文字・小文字の表。実行の段は例外も GC も使わない。
- **上限**（本家では `#define`）:

  | 名前 | 値 | 出どころ |
  |---|---|---|
  | STEP | 1,000,000 | 理由の記述なし |
  | STACK | 2048 | `(a)*?b` で測った最小値 |
  | PARSE_DEPTH | 4096 | Onigmo に合わせた |
  | 繰り返しの最大 | 32768 | |
  | 後読みの長さ | 255 バイト | |
  | 後ろ向き検索の窓 | 256 | |

  STACK と STEP は `Regexp::STACK_LIMIT`／`STEP_LIMIT` として Ruby から見える。

## SabiRuby 側の境目

- エンジンは `src/regexp/mod.rs`（908 行）。公開しているのは `compile`、`exec`、`rexec`、`charlen` と `Pattern` だけで、`regex_automata` を use しているのは 1 か所。
  - 約 740 行の「Ruby の構文を regex-syntax の構文に書き換える層」は、置き換えると要らなくなる。
- Ruby 側の層 `src/builtins/ext_regexp.rs`（1886 行）は regexp.c の写しで、ほぼそのまま使える。
  - 呼び出し元は `exec` が 6 か所、`rexec` が 1 か所、`compile` が 2 か所、`charlen` が 4 か所。
- 変える必要がある所が 2 つある。
  - `exec` が `Option` を返すので、上限超えを返す道が無い。`Result` にする。
  - 検索ごとの `binary` を捨てている（ext_regexp.rs:189-196, 211-218, 230-232）。これが「バイトとして読む subject」の KO の原因。

## 決めたこと（原則で決めた。理由つき）

- **3 つの器を全部写す（pike_vm も）。** 互換のため。本家で pike が受け持つパターンには段数の上限が無い。backtrack だけにすると、本家が答えるパターンで SabiRuby だけが「step limit over」を出す差が生まれる。速度のためではない（速度は目標ではない）。
- **`regex-automata`／`regex-syntax` への依存を外す。** 二段構え（断る構文だけ別のエンジンに回す）にはしない。同じ正規表現が 2 通りの意味を持つのを避ける。
- **`/i` の表は本家の unicase（Unicode 17.0.0）を写す。** Rust の `char::to_lowercase` は単純な畳み（simple fold）でも逆写像（unfold）でもなく、Unicode のバージョンも Rust のツールチェインで決まってしまう。表は本家の生成スクリプトを Ruby のまま使い、Rust の表を吐かせる。生成物は手で直さない。
- **上限は `Vm` の設定にする**（CLAUDE.md「数は利用者が変えられる場所に」）。
  - 既定値は本家の値で、`docs/numbers.md` に出どころとともに載せる。STEP の 1,000,000 は出どころ「不明（本家のコメントに無い）」と書く。
  - `RE_MAX_CAPTURES` 32 と `RE_MAX_CODE_LEN` 0xffff は表現の幅から決まる不変量なので、`const` のままにする。
- **unsafe は使わない。** C のポインタは添字にする。取り消し記録は 1 本の `Vec<i32>` に添字で並べる。`goto fail` は loop とラベル付きブロックにする。pike の `add_thread` の再帰は明示的なスタックにする（wasm の小さなスタックで溢れないように）。
- **`Pattern` に可変のキャッシュを持たない**（`Vm: Send + Sync`）。pike の作業領域は検索ごとに確保する。計測して必要なら、`Vm` の側に持たせる。
- 確保の失敗（本家の `RE_NOMEM`）は `try_reserve` で同じ形にする。

## 段階

各段で本家のテストの通過数を数え、前の段から減らないことを確かめる。段 1〜3 は今のエンジンのまま進める（regex-automata を外すのは段 4）。

| 段 | 中身 | 本家の範囲 | 確かめ |
|---|---|---|---|
| E0 | 土台。`exec`／`rexec` を `Result` に、検索ごとの `binary` を渡す、上限を `Vm` の設定にして `numbers.md` へ、`tests/regexp_engine.rs` の作り直しの準備 | regexp.c の `re_check_exec_error` | 挙動は変えない（今のエンジンのまま、テストの数が同じ） |
| E1 | 命令列とコンパイラの骨格。型、emit／patch／insert、levels のスタック、前処理（`(?#…)`、`/x`）、atom と量指定子、エラーの文言 | re_internal.h、re_compile.c:1-346, 2235-2464, 3037-4232 | 新しいコンパイラの単体テスト（本家と同じ文言で断るか） |
| E2 | 文字クラス。集合演算、POSIX、エスケープ、`\u`、ASCII と UTF-8 の区別 | re_compile.c:347-1838 | 同上 |
| E3 | `re_ctype` と unicase の表を、生成スクリプトで Rust に。ASCII だけのビルドの `re_cased` | re_ctype.h、re_cased.h、src/unicase.* | 表の項目数と、本家との抜き取り照合 |
| E4 | 照合の 3 つの器。literal、pike、backtrack の中心（SAVE、SPLIT、空ループ、STEP／STACK）、器の選び方、`rexec` の窓。**ここで regex-automata を外す** | re_exec.c 全部 | 今まで通っていたテストが全部通る。`cargo tree` から regex-* が消える |
| E5 | 先読み・後読み（固定長の計測を含む） | re_compile.c:1839-2234、re_exec.c の LOOK 系 | regexp_syntax の該当 |
| E6 | 後方参照・`\k`・条件分岐、アトミック・強欲な量指定子（BARRIER の cut） | re_compile.c:2465-3036 ほか | regexp_syntax の該当 |
| E7 | `\g` の呼び出しと不在演算子 | re_compile.c:4522-4836、re_exec.c:1637-1784 | regexp_call（15）、regexp_syntax の該当 |
| E8 | 解析と最適化。first set、anchor、前置文字列、空ループの印 | re_compile.c:4233-4521 | テストの数は同じ。正規表現のマイクロベンチ（新しく作る）で、E4 の後と比べる |
| E9 | 仕上げ。docs（gems.md の Deviations kept と設計の節、mrbtest-notes の engine の行、size.md の測り直し、numbers.md）、CHANGELOG、洗い出し（`tools/rc2_sweep`）の再実行 | — | mrbtest 3 ビルド、custom、洗い出しの diff、edge_compare |

- 段の切れ目は、本家のコードの塊に合わせた。E1〜E3 は互いに独立していて、並べて進められる。
- E5〜E7 は E4 の後なら、どの順でもよい。

## 守ること

- VM の crate に `unsafe` を入れない。`no_std + alloc` と `Vm: Send + Sync` を保つ。
- mrbtest（3 ビルド。今は 2346／2269／1981）を、どの段でも下げない。no-regexp のビルドの挙動は変えない（`Regexp` が無いまま）。
- 本家のテストは、本家の `mruby` コマンドの答えを正解とする（`mrbtest-notes.md` の「reference too」の行と同じ扱い）。
- 性能: 正規表現を使うベンチが今は無いので、E4 で正規表現のマイクロベンチを足す（`bench/micro-*` の形）。目的は、桁違いに遅くなっていないかを見ることだけ。27 本と既存のマイクロベンチは、A/B と A/A で今と変わらないことを確かめる（正規表現を使わないので変わらないはず）。
- wasm の大きさは size.md の手順で E4 と E9 に測って記録する（regex-automata の分 約 520 KB が消え、エンジンの分が足される）。

## 著者の判断（2026-10-19）

- 進めるのは後で（着手は著者の合図を待つ。POSIX の計画との順番もそのときに決める）。入れるバージョンもそのときに決める。

# 0.7.0 の計画: たまった直しを片付けてから出す

作成 2026-09-26。著者の判断（2026-09-26）:
- 「不具合や直すべき点をなるべく取り込んで、クリアにしてからリリースしたい」。
- 範囲は本体の案（この文書）でよい。
- 残したものは、後で参照しやすく整理する（[`../backlog.md`](../backlog.md)）。

0.7.0 は挙動が変わるバージョンになる。CHANGELOG の Unreleased の「ブロックの内側で待つ」（`wait-anywhere-plan.md`）がその中心で、次の S1〜S7 を加えて出す。rubevy 0.2.0（rubevy の `docs/plans/release-0.2-plan.md`）はこの 0.7.0 に上げる。

## 入れるもの

| # | 項目 | 種類 | 出どころ |
|---|---|---|---|
| S1 | **`Hash#[]`・`key?`・`fetch`・`delete` が、`eql?` の中で起きた例外を握りつぶす**（`hash_get` / `hash_delete` が `Option` を返すため）。本家は例外を出す | 不具合。公開 API の形が変わる | `../worklog/2026-09-23-rc2.md:151-160, 213`（案 a/b/c） |
| S2 | **`Vm::funcall` の「メソッドが無い」の分岐で、クロージャの `method_missing` を `call_closure` で呼ぶとき `direct_send` を下ろさない**。SEND から呼ばれたネイティブがそこへ `funcall` すると、入れ子なのに「SEND から呼ばれた」と見える | 不具合 | `../worklog/2026-09-26-wait-anywhere.md:531` |
| S3 | **rc2 で本家が入れた 2 つの確認に追いついているか**: 局所変数の数がレジスタの数を超える irep を読み込みで断る（`load.c`、`7ed51715e`）と、`OP_CALL` の受け手が Proc でないときの TypeError（`vm.c`、`78a055595`） | 本家との差なら揃える | `../worklog/2026-09-23-rc2.md:227-229` |
| S4 | **H2: irep が二度と返らない** | 振る舞い（メモリ） | `host-scale-plan.md` の H2 |
| S5 | **H5: `Exception#backtrace` にネイティブの名前が入らない**（`Vm::backtrace(Some(mid))` との食い違い） | 本家との差 | `host-scale-plan.md` の H5 |
| S6 | **H1: 待ちタスクの費用が O(N)** | 性能 | `host-scale-plan.md` の H1 |
| S7 | 片付け: H6（`check_no_std.sh` が `sabiruby-serde` を見ていない）、`eval-require-plan.md:9` の古い「未着手」、README の数を CI で確かめるか、docs に散らばったバージョンの例 | 道具・文書 | `host-scale-plan.md` の H6、`../worklog/2026-09-22-readmes.md:209`、`../worklog/2026-09-22-release-0.6.md:398` |

- 設計の分かれ道は、既にある計画書（`host-scale-plan.md` の H1・H2・H5 の節）の判断に従う。そこに無い分かれ道は、原則で決めて worklog の「決めたこと」に残す（`/home/kishima/book/CLAUDE.md`、止めずに記録して進める）。
- **S1 の案**（worklog の a/b/c）から選んだものと、その理由を書く。公開 API（`pub fn hash_get` など）を変えるなら CHANGELOG に。

## 守ること

- VM の crate に `unsafe` を入れない。`no_std + alloc`、`Vm: Send + Sync`、本家テストの基準（mrbtest の baseline、全ファイル）。
- **通常時の性能を落とさない**（wait-anywhere と同じ基準）。A/A のばらつきの幅を線にし、変更前後を交互に 2 巡以上（`tools/bench_ab.sh` は巡の中で A と B の順を入れ替えるようになった）。build をまたぐと、コードの配置でも揺れる（`../verification/bench.md`「After the author's decisions」）。命令数（`--stats`）が同じかどうかも並べる。
- **ベンチは本体が合図してから測る。** 同じ PC で複数の担当が動くので、ベンチの時間を本体が割り振る。それまでは build とテストだけ進める。

## 段階

| 段階 | 中身 | 担当 |
|---|---|---|
| A | S1・S2・S3・S7 | 実装担当 1（ブランチ `rel07-correct`） |
| B | S4・S5・S6 | 実装担当 2（ブランチ `rel07-scale`、別の worktree） |
| C | A と B を main に取り込み、全部のテスト・mrbtest・no_std、ベンチ（本体の合図で）、CHANGELOG を 0.7.0 に | 本体 + 実装担当 |
| D | 公開: `cargo publish`（`sabiruby`、必要なら `sabiruby-compiler`・`sabiruby-serde`・`sabiruby-cli`）と tag | **著者の確認の後に**（外に出す操作） |

## 状況

| 段階 | 状況 |
|---|---|
| A | 未着手 |
| B | 未着手 |
| C | 未着手 |
| D | 未着手 |

# C mruby が書き出した実行状態を、SabiRuby で続きから動かす計画

作成 2026-11-02（案。作者向け）。feature `checkpoint`（既定 off）として、このブランチに
最初の段（C mruby → SabiRuby の片道）を実装した。この文書は計画と、その段の実装の記録を兼ねる。
第 3 段（2026-11-03）で形式 v1（Task と Fiber）に合わせた。

## 目的

C mruby（4.0 系）で動いているプログラムを途中で止め、状態（ヒープ・フレーム・
レジスタのスタック）を書き出したもの（以下「イメージ」）を SabiRuby が読み、止まった命令の
次から動かす。

- 書き出す側は C mruby の gem（mruby-checkpoint。このリポジトリの外）。プログラムの中の
  `Checkpoint.save(path)`（書いて動き続ける）・`Checkpoint.suspend(path)`（書いて終わる）が、
  呼び出しの直後の命令の境目で状態を書く。
- イメージは C の構造体の写しではなく、mruby の意味論の層の「論理グラフ」: オブジェクトに通し
  番号を振り、参照は番号、シンボルは名前、コード（irep）は中身（とその FNV-1a 64 の
  ハッシュ）、書き出し側の VM が最初から持っていたもの（組み込みのクラス、mrblib の
  メソッド、C の関数）は名前で運ぶ。値は Integer 64 bit・Float 倍精度・String は UTF-8 に
  固定で、ポインタ幅に依らない（64 bit と 32 bit の C mruby が書くイメージは、書き出し側のポインタ幅を
  記す 1 バイト以外同一）。
- SabiRuby で読めることが、「形式が C mruby の内部に依存していない」ことの確かめになる。

## 決めたこと

- **feature `checkpoint`（既定 off）**に置く。off のビルドにはこのコードが一切入らない
  （`src/checkpoint/` は `#[cfg(feature = "checkpoint")]` のモジュール、`src/vm.rs` に足したのは
  cfg 付きの関数 1 つだけ。第 3 段で `src/builtins/ext_task.rs` に cfg 付きの関数 3 つ: イメージの Task を
  キューに入れる、実行中だった Task のタイムスライスの残りを続ける、スケジューラのループを回す）。命令の実行・GC・`Value` の表現は変えない。no_std のまま
  （`tools/check_no_std.sh --features checkpoint` も通る）。
- CLI（`sabiruby-cli` の feature `checkpoint`）に `--resume IMAGE` と
  `--checkpoint-names FILE`（後述）を足す。
- **逆向き（SabiRuby → C mruby）はこの段ではやらない。** SabiRuby の
  `Checkpoint.save` / `suspend` は何も書かず、書き出し側の「基準の実行」（`--ignore`）と同じ値を
  返す（`save` は `:saved`、`suspend` は `:resumed`）。2 回 suspend するプログラムは、最初のイメージ
  から SabiRuby で最後まで動く。

## 足した API

| API | 中身 |
|---|---|
| `Vm::checkpoint_resume(&mut self, image: &[u8]) -> VmResult<Value>` | イメージを組み立てて、止まった所から最後まで動かす。戻り値はトップレベルの値。組み立てられないイメージは `Checkpoint::Error`（"restore failed: 理由"） |
| `Vm::checkpoint_install(&mut self)` | `Checkpoint` モジュール（`save`・`suspend`・`resumed?`・`Error`・`Box`）を定義する。resume が最初に呼ぶ |
| `Vm::checkpoint_name(&mut self, name) -> Result<&str, String>` | 書き出し側の permanent の名前 1 つが、ここで何に当たるか |
| `Vm::checkpoint_header(image) -> Result<(u16, String), String>` | 形式の版と書き出し側の名前 |
| `checkpoint::image_counts`、`checkpoint::data_types` | 道具向け（イメージの各表の数、C のデータの型名で読めるもの） |
| `Vm::checkpoint_run`（`pub(crate)`、`src/vm.rs`） | 組み立てたフレームを `run_loop(0)` で動かし、終わったらスタックを空にする（`run_irep` の後半と同じ） |
| `sabiruby --resume IMAGE` | `run` と同じ VM（mrblib、ARGV、`$DEBUG`）を作って resume |
| `sabiruby --checkpoint-names FILE` | 書き出し側の `checkpoint names` の出力を読み、名前ごとに `ok`／`missing` と理由、接頭辞ごとの数 |

VM の内部の型（`VmIrep`・`CallInfo`・`EnvData`・`ProcData`・`ClassData`・`Heap::alloc`）は、
crate の中の今の公開範囲のまま使っている。新しく公開した内部は無い。

## 組み立ての手順（`src/checkpoint/`）

1. `image.rs`: イメージ全体を読んで、素のレコードにする（ヘッダの検査: マジック `MRCK`、形式の版（第 3 段から 1）、
   `RITE0400`、64/64/UTF-8。余りのバイトがあれば断る）。
2. `build.rs`、GC を止めたまま（`gc_disabled`）:
   1. シンボルを intern する。
   2. irep を `Vm::ireps` に足す。運ばれた中身から作り、ハッシュを計算し直して照合する。
      ハッシュだけの permanent な irep（中身なし）は断る（SabiRuby の mrblib は 4.1.0-rc2 製で
      中身が違うため、必ず中身が要る）。
   3. オブジェクトを 2 回なめる: 1 回目で全部の器を作り（permanent は名前で引く）、2 回目で
      中身を埋める。
   4. permanent なものへの変更（メソッドの定義・削除、ivar・定数・クラス変数、スーパークラスの付け
      替え＝permanent なクラスへの include、グローバル変数）を当てる。
   5. 入れ子のクラスの名前（`A::B` の文字列）から `outer` を決める。
   6. Hash の項目を入れる（キーの `hash`／`eql?` はプログラムのメソッドでもよいので、全部が
      そろってから `hash_set` で）。
   7. frozen のフラグを立てる。
   8. スタックとフレームを組み立てる。
3. `Vm::checkpoint_run` で、いちばん上のフレームの pc から動かす。

### 写し方

| イメージ | SabiRuby |
|---|---|
| フレーム（callinfo）: 窓の始まり、pc、`n`、`nk`、可視性の 4 bit、環境（Env か定義先のクラス） | `CallInfo`。`kw` は `nk == 15`（ENTER の後は 0 か 15。それ以外は断る）。いちばん下のフレームは `Cci::Skip`（`run_irep` と同じ）、他は `Cci::None`。C から入ったフレーム（`cci != 0`）は断る（書き出し側も書かない）。`method_block` は false |
| フレームの pc | そのまま（いちばん上は次の命令、下は呼び出しの次の命令。SabiRuby の `exec_frames` と同じ意味） |
| フレームの渡されたブロック（`ci->blk`） | 使わない。ブロックはフレームのブロックのレジスタにもあり、SabiRuby はそこを読む |
| スタック上の Env（位置で書かれる） | `EnvData { attached: true, base: 位置, ctx: ROOT }`。位置がどれかのフレームの始まりで、そのフレームの環境であることを確かめる |
| 切り離し済みの Env | `values` |
| Proc（strict・orphan・scope、本体の irep、upper、環境） | `ProcData`。環境が Env なら定義先のクラスは Env のクラスフィールドから |
| クラスの「ivar」（定数・クラス変数・`__classname__`・`__outer__`・`__attached__` が 1 つの表） | 定数は `consts`、`@@x` は `cvars`、`__classname__` は `name`（Symbol ならトップレベル、String のパスなら最後の部分と `outer`）、`__attached__` は `attached`、残りは ivars |
| 取り込みクラス（クラスフィールドが取り込んだモジュール） | `ClassData { iclass_of }` |
| メソッドの項目: Proc / C 関数の名前 / undef、可視性 | `Method::Ruby` / 名前で引いた SabiRuby のメソッド（Native でも Ruby でもそのまま）/ `Method::Undef`、`vis` |
| `attr_reader`／`attr_writer` の Proc（C 関数 + ivar 名を持つ Env） | `Method::AttrReader(ivar)`／`AttrWriter(ivar)` |
| Hash の既定値（ivar `ifnone` + フラグ） | 既定値は `HashData::default`、既定の Proc は ivar `__default_proc` |
| Exception のメッセージとバックトレース（文字列の配列） | ivar `mesg`、`@__btstr`（`set_backtrace` と同じ置き場） |
| クラスを持たないオブジェクト（C の Proc、C 関数が作る内部の配列。レジスタに残る） | Proc・Array・String・Hash の素のクラス |
| C のデータ（型名 + バイト列） | 型名ごとの復元の関数（今は `Checkpoint::Box` だけ。値を Data の handle に持つ） |

### mrblib のフレーム: 中身ごと運ぶ

SabiRuby の mrblib は 4.1.0-rc2 の mrbc で作られていて、書き出し側（4.0 系）の mrblib とは
irep の中身が違う。そのため、`Integer#times` や `Array#each` の中（ブロックの中で止まった）
のフレームは、名前で SabiRuby のメソッドに結んでも pc が合わない。

書き出し側は、表に入る mrblib の irep（フレームが動かしているもの、実行中に作られた Proc が持つ
もの、その子）を中身ごと書く（irep の種類 2）。フレームの Proc が mrblib のメソッドなら、名前と
irep の番号を書く（オブジェクトの種類 15）。SabiRuby は、その irep を本体とする Proc を
新しく作ってフレームに使う（持ち主のクラスは名前から、strict・scope）。その Proc を `upper` に
持つブロックも同じものを指す。フレームが戻ると、それ以降はふつうに SabiRuby のメソッドが呼ばれる。

イメージの大きさへの影響は、mrblib のフレームがあるときだけ。試験のイメージで 59〜254 バイト
（`Integer#times` 1 つで 93 バイト）。

## 名前の対応

SabiRuby は対応の表を持たない。名前を自分のクラスをたどって引く（`src/checkpoint/names.rs`）:
`T` は main、`Integer`・`Math::DomainError` は Object からの定数のパス、`S:x` は x の特異
クラス（無ければ作る）、`I:x/k` は x の直上 k 番目の取り込みクラス、`M:x#m`・`F:x#m` は x の
メソッド表（無ければ祖先から）、`E:x#m` はその Proc の Env、`V:x@v` はクラスの ivar・定数・
クラス変数の値（`@__classname__` は名前の文字列を作る）、`G:$v` はグローバル変数。`X:`（書き出し側が
前もって作る例外）には対応が無い。

書き出し側の名前 1554 個（C mruby 4.0 系の既定の gembox + mruby-task + mruby-checkpoint）を
`--checkpoint-names` で引いた結果:

| 接頭辞 | ある | 無い |
|---|---:|---:|
| クラス・モジュール | 70 | 24 |
| `S:` | 76 | 24 |
| `I:` | 13 | 4 |
| `M:` | 334 | 107 |
| `F:` | 712 | 143 |
| `E:` | 0 | 13 |
| `V:` | 19 | 13 |
| `X:` | 0 | 2 |

- 無い 330 個のうち 309 個は、SabiRuby に無い POSIX の gem（mruby-io・socket・dir・errno と
  `ENV`・`STDOUT` など。[`posix-gems-plan.md`](posix-gems-plan.md)）のもの。
- `M:` のうち 28 個は、C mruby では Ruby、SabiRuby では Rust のメソッド（`Kernel#puts` など
  mruby-io が Ruby で書き直すもの、`String#gsub` など mruby-regexp のもの）。メソッド表の項目
  としては結べる。Proc を値として持っていると断る。
- 残りの 21 個: C mruby の内部の補助（`Kernel#__to_int`、`Regexp#__sub_str` など）、
  `NoMemoryError`、`attr_reader` で作られた C mruby の Proc の Env（`E:NameError#name` など。
  SabiRuby では属性メソッドなので Env が無い）、`Random@mruby_Random`、`X:nomem_err`・
  `X:stack_err`。

## 命令の集合と意味の差（4.0 系 ↔ 4.1.0-rc2）

- 命令の表（`include/mruby/ops.h`）は 119 個とも名前・番号・オペランドの形が同じ。
  書き出し側がコンパイルしたプログラムの irep も、運ばれた mrblib の irep も、そのまま動く。
- 例外処理の表（13 バイト）、プールの種類、シンボルの扱いも同じ。
- 差が出たのは命令ではなく、mrblib・ライブラリの作りの違い:
  - **`Kernel#catch`**: 書き出し側は bytecode の Proc（`catch_proc`）で、`throw` は呼び出しの
    フレームを遡ってその Proc を探す。SabiRuby の `catch` は native のループのフレーム（`ext_catch.rs`）で、
    `throw` はそれを探す。運ばれた `catch` のフレームは SabiRuby の `throw` に見つからない。
    → そのフレームを含むイメージは断る（"a frame runs Kernel#catch"）。
  - **Method オブジェクト**（`3.method(:+)` など）: 書き出し側は中に C 関数の Proc を持つ。
    SabiRuby の Rust のメソッドは Proc ではない。→ 断る（"a Proc made from the C function …"）。

## 試験

`tests/checkpoint.rs`（feature on のときだけ）。`tests/checkpoint/` に、書き出し側の試験の
サンプル（`.rb`）と、それを C mruby で動かして書いたイメージ（`.ckpt`。2 回以上 suspend するものは
チェーンの各点のイメージ、32 bit の書き出し側のものも 1 つずつ）と、期待する出力（`.out`。書き出し側の基準の
実行のその点より後）か断りの文言（`.refused`）を置いた。

| 試験 | 確かめること |
|---|---|
| `images_resume` | 全部のイメージ（29 個）が、`.out` と同じ出力で最後まで動くか、`.refused` の理由で断られる |
| `refused_leaves_vm_idle` | 断られた後の VM で別のイメージが動く |
| `truncated_images_are_refused` | イメージをどこで切っても、panic せずに断る |
| `changed_code_is_refused` | 運ばれたコードを 1 バイト変えると、ハッシュの照合で断る |
| `image_is_the_same_from_32_bit` | 64 bit と 32 bit の書き出し側のイメージの違いは 1 バイト |

結果: feature off・on の両方で `cargo test --workspace` が通る。`SABIRUBY_GC_STRESS=1` でも
`tests/checkpoint.rs` は通る。`tools/check_no_std.sh` は feature 無しでも `--features checkpoint`
でも通る。

イメージの内訳（サンプルごと）:

| サンプル | 中身 | SabiRuby |
|---|---|---|
| counter | while の途中、2 回 suspend | 2 点とも続く |
| block | `times`・`each_with_index`・入れ子の `each` のブロックの中（mrblib のフレーム） | 3 点とも続く |
| method_middle | メソッド 3 段の途中、省略可能・残り・キーワード引数 | 続く |
| closure | 外の変数を書き換えるクロージャ（スタック上と切り離し済みの Env） | 2 点とも続く |
| user_class | attr_accessor、継承、include（自作と Comparable）、クラスメソッド、クラス変数、定数、特異メソッド | 続く |
| builtin_patch | String・Integer へのメソッド追加、Array の C メソッドの別名、トップレベルの def、main の ivar、グローバル変数 | 続く |
| hash | 色々なキー（`hash`／`eql?` を持つ自作のキー）、既定値、既定の Proc、Range | 続く |
| rescue | rescue 節の中（例外がレジスタにある）、ensure、retry | 2 点とも続く |
| data_box | C のデータ（`Checkpoint::Box`） | 続く |
| save | `save` の戻り値（復元した側は `:resumed`）、`each` の中、メソッド 2 段 | 続く |
| c_boundary | C の関数の外に出た後の suspend | 続く |
| sabiruby_refuse | `catch` の中、Method オブジェクトを持つ | 2 点とも断る |

## 断るもの

| 場面 | 理由 |
|---|---|
| ハッシュだけの permanent な irep | 中身が要る（mrblib が違う） |
| 運ばれた irep のハッシュが合わない | 壊れている |
| SabiRuby に無い permanent の名前を、値として指している | 何に結ぶか無い（POSIX の gem など）。メソッドの項目なら Native にも結ぶ |
| C 関数の Proc を値として持つ（Method オブジェクトなど） | Rust のメソッドは Proc でない |
| `Kernel#catch` のフレーム | `throw` が見つけられない |
| 型名の分からない C のデータ | 復元の関数が無い（今は `Checkpoint::Box` だけ） |
| ENTER の前のキーワード引数（`nk` が 1〜14） | SabiRuby のフレームはキーワードを Hash 1 つで持つ |
| C から入ったフレーム | 書き出し側も書かない（形式の約束） |

書き出し側が断るもの（Fiber・Task の中、Struct・Set などの型、prepend、C の関数が呼んだ
ブロックの中）はイメージにならない。

## 既知の限り（この段では直していない）

- **古いレジスタ**: フレームの窓の中に、もう使われない値が残っていることがある。書き出し側が
  第 2 段で命令列から生きているレジスタを求め、死んだ一時レジスタを nil で書くようになった
  （下の「第 2 段」）。読む側は何もしない。
- フレームの Proc（mrblib のメソッド）は、新しく作った Proc が代わりに立つ。`Method#==` などで
  その Proc の同一性を見るプログラムは違いが出うる。
- 断ったときの VM: 変更表を当てた後で断った場合（Hash のキーの `hash` が例外を出した、フレームの
  検査で断った）は、permanent なクラスへの変更が残る。ホストはその VM を捨てる前提。
  切れたイメージ・版の違いなどは読む段で断るので、何も変わらない。
- 行番号: 第 2 段で形式に任意のセクション `DBG0` が入り、運んだ irep に付けるようにした。
- イメージは `.mrb` と同じく信頼する入力として扱う。ハッシュの照合は事故の検出で、改ざんの防止
  ではない。命令列の中身の検査はしていない（`load` と同じ）。

## 第 2 段: 形式の追補に合わせる（2026-11-03）

書き出し側が、外から止めた試験一式（mruby 付属の試験をランダムな命令の境目で止める）を
運ぶために形式へ足したものに、このリーダーを合わせた。

| 追補 | このリーダー |
|---|---|
| VM の後の任意のセクション（`DBG0` 行番号、`GCS0` GC の設定）と終端タグ `END0` | `DBG0` は運んだ irep の `lines`・`filename` に当てる。`GCS0` は読み飛ばす（GC の作りが違う）。`END0` が無ければ切れたイメージとして断る |
| 本体の種類 2 の Proc（`alias_method` の別名） | メソッド表の項目としては元のメソッド（`upper`）に結ぶ。値として要るときは断る |
| 種類 25 の String（`String#b` のフラグ） | `binary` を立てた String |
| C のデータ `Time` | 秒・ナノ秒・時間帯から、この VM の Time（隠れた ivar）を作る |
| 変更表の種類 10（permanent な String・Array の中身。`ARGV << x`） | 中身を入れ替える。Hash は断る |
| 子の数があって子の表が無い irep（書き出し側の mrblib の一部） | 子を結ばない |
| 種類 16〜24（bigint・Struct・Set・Rational・Complex・ensure を通る break・Random・prepend の origin）、変更表の種類 8・9 | まだ運ばない。何が無いかを添えて断る |

CLI は `-r lib` を `--resume` のときも先に読む（ホストの準備。試験一式を続けるときは、書き
出し側の試験の環境 `t_print` などを Ruby で定義したものを渡す）。

試験のイメージ（`tests/checkpoint/`）は、この追補を含む書き出し側で作り直した（中身の試験は
同じ）。

書き出し側の試験一式の一部（中核の試験 37 本、1 本 20 点）を、ここで続けたもの: 740 点
のうち書き出し 729、ここで断ったもの 13（prepend の origin 12、ensure を通る break 1）、
続いた 716 はすべて往復なしの出力と一致した。中核の試験の
うち 6 本は、イメージなしで最初から動かしてもこの VM と書き出し側の出力が違うので外した:
`codegen.rb`（`class Regexp; end` の後の正規表現リテラル）、`gc.rb`（`GC.malloc_threshold=`）、
`string.rb`（`String#index` の UTF-8）、`vformat.rb`（書き出し側の C の試験関数）と、比べ方の
都合の 2 本（`range.rb`・`syntax.rb`: プログラムのファイル名が両者で違い、バックトレースと
`__FILE__` に出る）。

## 第 3 段: 形式 v1（Task と Fiber）（2026-11-03）

書き出し側が形式を v1 にした。v0 の VM の部分（ルートのコンテキスト 1 つ）が、実行のコンテキストの表（ルート・
Task ごと・Fiber ごとのスタックとフレーム）、今のコンテキスト、スケジューラ（Task をキューの順に、時計、待ちの残りの
tick）になった。書き出し側は Fiber の中・Task の中・Task が待っている最中でも書き、断るのは C の
関数のフレームだけになった。このリーダーを v1 に合わせ、Fiber と Task を組み立てて続けるようにした
（片道のまま）。v0 のイメージは読まない。

### 写し方（第 3 段で足したもの）

| イメージ | SabiRuby |
|---|---|
| コンテキストの表の 1 番（ルート） | `contexts[ROOT]`。いちばん下のフレームは `Cci::Skip`（第 1 段と同じ） |
| ほかのコンテキスト（Task・Fiber） | `contexts` に 1 つずつ足す。`prev`、`fib`、`proc_`（いちばん下のフレームの Proc） |
| コンテキストの状態 | `FiberState` にそのまま。ただし、書き出し側のルートと Task のコンテキストは動いた後も「未開始」(0) のことがあるので、ルートは `Running`、Task のコンテキストは `Suspended` にする。未開始の Fiber（フレーム 2 枚がどちらも pc 0）は `Created` にし、フレームを持たせない（resume でブロックを並べる、この VM のやり方） |
| 待っているコンテキストのいちばん上の C の関数のフレーム（`Fiber#resume`・`Fiber#transfer`・`Fiber.yield`） | フレームにしない。そのフレームの窓の始まりのレジスタを `pending_reg` にする（切り替えで戻ったときの値が入る所） |
| 終了したコンテキスト | スタックもフレームも持たない。そのコンテキストのスタック上の Env は、イメージのスタックの値で切り離す（書き出し側の Task は終わっても Env を切り離さない。この VM の `stop_task` は切り離す） |
| スタック上の Env（コンテキストの番号 + 位置） | `EnvData { ctx, base, attached }` |
| オブジェクトの種類 27（Fiber） | `ObjKind::Fiber(コンテキスト)` |
| オブジェクトの種類 26（Task）とスケジューラの Task の表 | `ObjKind::Task(TaskData)`。状態・理由・優先度・名前・値・待ち先（join の Task、Queue のオブジェクト）・起きる時刻（今の tick + 残り）。キューはイメージの順に入れる（`ext_task::checkpoint_enqueue`） |
| main の Task（`Task.current` をルートで呼んで作られたもの） | `vm.task.main`。名前は `"main"` |
| C のデータ `Task::Queue` | ivar `@items`（イメージの ivar）と `@closed`（イメージの 1 バイト）を持つ、この VM の Queue のオブジェクト |
| 時計 | `tick` を書き出し側の tick にする（1 tick のミリ秒が違えば換算）。待ちは残りの tick で運ばれ、今の tick に足す |

### 続け方

| イメージ | ここで |
|---|---|
| Task が動いていない（今のコンテキストはルートか、ルートから resume された Fiber） | 今のコンテキストをこの VM の `stack`／`ci` に置いて、ルートのループ（`run_loop_ctx(ROOT, 0)`）を回す。Fiber が yield・終われば、`pending_reg` に値が入ってルートが続く |
| Task が実行中だった | その Task のタイムスライスの残りを、今のコンテキスト（Task のコンテキストか、その Task が resume した Fiber）から続ける（`ext_task::checkpoint_slice`。`execute_task` と同じ手順で、始まりのコンテキストだけが違う）。スケジューラがその境目で切り替えを要求していたなら、続けずにタイムスライスの後の扱いだけをする。その後、スケジューラを入れた側に戻る |
| ↳ スケジューラを `Task.run` が回していた | 呼んだ側の pc を `Task.run` の呼び出しの命令に戻して、呼び出しをやり直す（ループはキューの今の状態から回る） |
| ↳ ルートの `Task.pass` | その 1 回は終わっている。呼び出しの値は nil |
| ↳ ルートにフレームが無い（書き出し側の埋め込む側がスケジューラを回していた） | ループ（`Task.run` と同じもの）を回す（`ext_task::checkpoint_run_loop`） |

タイムスライスは、着いた先で満タンから始まる（`execute_task` と同じ）。第 4 段で、イメージに記された
残りから続けるようにした（下）。

### Task・Fiber の作りの違い（書き出し側 ↔ ここ）

| 項目 | 書き出し側（C mruby + mruby-task） | ここ |
|---|---|---|
| tick | タイマの割り込み（4 ms）。書き出し側の試験のビルドは仮想の時計（空いたとき・ルートの sleep で進む） | 命令の数（既定 10,000 命令で 1 tick）。空いたときは次の起きる時刻へ飛ぶ |
| 待ちの期限が過ぎた Task が起きる時 | 次の tick | 次の tick（命令の数で来る） |
| コンテキストの切り替えの値 | 待っている C の関数のフレームの窓の始まりに、戻ってきた側が書く | `pending_reg` に `deliver` が書く |
| Task のコンテキストの状態 | 動いた後も「未開始」のまま | 動けば `Created` でなくなる |
| 終了した Task のコンテキスト | フレームと Env をそのまま残す | 切り離して空にする |
| Task の中の `Fiber#transfer` | できる | FiberError（Task のコンテキストは自分のループで動く）。イメージに関係なく違う |
| Task の中の `Fiber.current` | Task のコンテキストを指す Fiber。その Fiber が回収されると、Task の中のコンテキストを解放しようとする（書き出し側の不具合。終わりに落ちる） | コンテキストの番号を持つだけ |
| Task の中でタイマで止められた Fiber | その Task は次に動くとき、いちばん上のフレームに Proc が無いので DORMANT にされる（書き出し側の不具合） | （未確認） |

### 試験（第 3 段）

`tests/checkpoint/` に、書き出し側の Task・Fiber のサンプル（`task_*`・`fiber_*`・`lines`）のイメージを
足し、Task として起動した形（書き出し側の `checkpoint run --as-task`: プログラムの本体を Task に
して埋め込む側がスケジューラを回す）のイメージを `NAME-task.K` として置いた（イメージ 91 個、約 240 KB）。
`task_switch` は、書き出し側のスケジューラが切り替えを要求している境目のイメージ（タイムスライスがそこで終わる）。
`refused_leaves_vm_idle` は、Task のイメージを途中で断った後の VM が新しい VM と同じに動くことも確かめる。
`image_is_the_same_from_32_bit` は Task・Fiber のイメージも見る。

結果: feature off 276 passed、on 281 passed、`SABIRUBY_GC_STRESS=1` で `tests/checkpoint.rs` の 5
passed、`tools/check_no_std.sh` は off・on とも OK。書き出し側の試験一式を外から止めて続けたもの:
中核の試験（書き出し側と同じ出力になる 37 本 × 20 点）は 740 点のうち書き出し 731、ここで断った
13（第 2 段の未対応の種類）、続いた 718 はすべて一致。mruby-fiber・mruby-task・mruby-enumerator の
試験（同じく、イメージなしで同じ出力になるもの）も合わせて、シード 1〜3 の 2,400 点のうち書き出し 2,375、
ここで断った 37（未対応の種類とキーの hash の副作用）、ハッシュの値を書き出し側とここで別々に計算して
比べる試験（`Array#hash`）1、続いた残りはすべて一致。

この段で直したもの: フレームが動かす mrblib のメソッドの代わりの Proc（第 1 段）に、外側の Proc（クラスの
本体の並び）を付けていなかったので、そのフレームから外側のクラスの定数が引けなかった
（`Enumerator::Generator#each` の `Yielder`）。この VM の同じ名前のメソッドが Ruby なら、その外側を
付ける。

## 第 4 段: タイムスライスの残り、mrblib の行番号（2026-11-03）

形式は v1 のまま（バイトの並びは変わらない）。書き出し側の 2 つの変更に合わせた。

| 書き出し側 | ここ |
|---|---|
| 実行中だった Task のタイムスライスは、イメージに記した残りから続ける（形式の §12。残りの tick と同じ考え方で、同じイメージから何度続けても同じ動きになる） | `ext_task::checkpoint_slice` はタイムスライスを満タンにせず、イメージの値（組み立てで Task に入れたもの）のまま続ける。0 も 0 のまま（tick はそのタイムスライスを終わらせない。書き出し側と同じ）。1 tick の中の命令の数はイメージに無いので、tick の境目から数え直す |
| `DBG0` に、運ぶ mrblib の irep（permanent、中身つき）の行番号も書く。書き出し側の mrblib は行番号つき（`mrbc -g`）で、ファイル名はソースツリーからの相対（`mrblib/array.rb` など） | 変更なし。もともと `DBG0` の行番号を運んだ irep すべてに当てていた。ここは mrblib の irep も中身から作るので、フレームが動かしている mrblib のメソッド（`Array#each` など）の行が、復元の後のバックトレースに出る |

ここの mrblib は行番号なしで作られているので、復元の後にここで新しく呼んだ mrblib のメソッドのフレームは
バックトレースに行が出ない（書き出し側では出る）。イメージから運んだフレームにだけ行が出る。

試験のイメージを作り直した（行番号のセクションが mrblib の分だけ増えた）。`lines_mrblib` を足した: `Array#each` が
呼んだブロックの中で書き出し、復元の後に起こした例外のバックトレースに `mrblib/array.rb:20:in each`
が出る（イメージの行番号から）。イメージ 95 個、約 260 KB。

## 残り（逆向き: SabiRuby → C mruby で要りそうなこと）

- 書き出す場所: SabiRuby には C mruby の `task.switching` に当たるセーフポイントがすでにある
  （`exec_frames` の先頭の 4 つの判定）。`Checkpoint.save` のフラグをそこに相乗りさせる形が自然。
- 名前を付ける側: 逆向きでは SabiRuby が自分の組み込みに C mruby と同じ名前を付ける。この段の
  「名前を引く」手順の逆（クラスの表をなめて名前を付ける）が要る。
- SabiRuby の native のループのフレーム（`push_loop_frame`。`catch`、`sort { }`、`Array.new(n) { }` など）は
  C mruby では C の関数のフレームで、運べない。逆向きでは断る場面になる。
- `Method::Closure`・Data（handle）・HostStore の値は C mruby に対応が無い。
- Value の `Slot` と `Env` の持ち方は形式とそのまま対応する（この段で確かめた）。

## 作者に決めてほしいこと

1. feature の名前と置き場（`checkpoint`、VM の crate の中）でよいか。別の crate にする手も
   ある（VM の内部の型を使うので、今は中に置いた）。
2. `Checkpoint` モジュールを SabiRuby 側でどこまで持つか（今は resume のときだけ定義し、
   書き出しはしない）。
3. 試験のイメージ（`tests/checkpoint/*.ckpt`、約 55 KB）を置くことと、それを作る書き出し側が
   このリポジトリの外にあること。

## 第 5 段: 任意のセクションの長さ（2026-11-03）

形式は v1 のまま、任意のセクションの並びだけ変わった: 各セクションは「タグ 4 バイト + `u32` 中身の長さ + 中身」
（終端タグ `END0` は長さを持たない）。知っているセクション（`DBG0`・`GCS0`）は長さで区切って読み、中身が
長さとちょうど一致しなければ断る。知らないタグのセクションは長さぶん読み飛ばす（後から書き出し側がセクションを
足しても読める）。試験のイメージを作り直し、知らないセクションを挟んだイメージが同じ出力で続くこと、長さの合わないセクションを
断ることの試験を足した。

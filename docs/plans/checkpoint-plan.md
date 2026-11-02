# C mruby が書き出した実行状態を、SabiRuby で続きから動かす計画

作成 2026-11-02（案。作者向け）。feature `checkpoint`（既定 off）として、このブランチに
最初の段（C mruby → SabiRuby の片道）を実装した。この文書は計画と、その段の実装の記録を兼ねる。

## 目的

C mruby（4.0 系）で動いているプログラムを途中で止め、状態（ヒープ・呼び出しの枠・
レジスタのスタック）を書き出したもの（以下「像」）を SabiRuby が読み、止まった命令の
次から動かす。

- 書き出す側は C mruby の gem（mruby-checkpoint。このリポジトリの外）。プログラムの中の
  `Checkpoint.save(path)`（書いて動き続ける）・`Checkpoint.suspend(path)`（書いて終わる）が、
  呼び出しの直後の命令の境目で状態を書く。
- 像は C の構造体の写しではなく、mruby の意味論の層の「論理グラフ」: オブジェクトに通し
  番号を振り、参照は番号、シンボルは名前、コード（irep）は中身（とその FNV-1a 64 の
  ハッシュ）、書き出し側の VM が最初から持っていたもの（組み込みのクラス、mrblib の
  メソッド、C の関数）は名前で運ぶ。値は Integer 64 bit・Float 倍精度・String は UTF-8 に
  固定で、ポインタ幅に依らない（64 bit と 32 bit の C mruby が書く像は、書き手のポインタ幅を
  記す 1 バイト以外同一）。
- SabiRuby で読めることが、「形式が C mruby の内部に依存していない」ことの確かめになる。

## 決めたこと

- **feature `checkpoint`（既定 off）**に置く。off のビルドにはこのコードが一切入らない
  （`src/checkpoint/` は `#[cfg(feature = "checkpoint")]` のモジュール、`src/vm.rs` に足したのは
  cfg 付きの関数 1 つだけ）。命令の実行・GC・`Value` の表現は変えない。no_std のまま
  （`tools/check_no_std.sh --features checkpoint` も通る）。
- CLI（`sabiruby-cli` の feature `checkpoint`）に `--resume IMAGE` と
  `--checkpoint-names FILE`（後述）を足す。
- **逆向き（SabiRuby → C mruby）はこの段ではやらない。** SabiRuby の
  `Checkpoint.save` / `suspend` は何も書かず、書き出し側の「基準の実行」（`--ignore`）と同じ値を
  返す（`save` は `:saved`、`suspend` は `:resumed`）。2 回 suspend するプログラムは、最初の像
  から SabiRuby で最後まで動く。

## 足した口

| 口 | 中身 |
|---|---|
| `Vm::checkpoint_resume(&mut self, image: &[u8]) -> VmResult<Value>` | 像を組み立てて、止まった所から最後まで動かす。戻り値はトップレベルの値。組み立てられない像は `Checkpoint::Error`（"restore failed: 理由"） |
| `Vm::checkpoint_install(&mut self)` | `Checkpoint` モジュール（`save`・`suspend`・`resumed?`・`Error`・`Box`）を定義する。resume が最初に呼ぶ |
| `Vm::checkpoint_name(&mut self, name) -> Result<&str, String>` | 書き出し側の permanent の名前 1 つが、ここで何に当たるか |
| `Vm::checkpoint_header(image) -> Result<(u16, String), String>` | 形式の版と書き手の名前 |
| `checkpoint::image_counts`、`checkpoint::data_types` | 道具向け（像の各表の数、C のデータの型名で読めるもの） |
| `Vm::checkpoint_run`（`pub(crate)`、`src/vm.rs`） | 組み立てた枠を `run_loop(0)` で動かし、終わったらスタックを空にする（`run_irep` の後半と同じ） |
| `sabiruby --resume IMAGE` | `run` と同じ VM（mrblib、ARGV、`$DEBUG`）を作って resume |
| `sabiruby --checkpoint-names FILE` | 書き出し側の `checkpoint names` の出力を読み、名前ごとに `ok`／`missing` と理由、接頭辞ごとの数 |

VM の内部の型（`VmIrep`・`CallInfo`・`EnvData`・`ProcData`・`ClassData`・`Heap::alloc`）は、
crate の中の今の公開範囲のまま使っている。新しく公開した内部は無い。

## 組み立ての手順（`src/checkpoint/`）

1. `image.rs`: 像全体を読んで、素のレコードにする（ヘッダの検査: 印 `MRCK`、形式の版 0、
   `RITE0400`、64/64/UTF-8。余りのバイトがあれば断る）。
2. `build.rs`、GC を止めたまま（`gc_disabled`）:
   1. シンボルを intern する。
   2. irep を `Vm::ireps` に足す。運ばれた中身から作り、ハッシュを計算し直して照合する。
      ハッシュだけの permanent な irep（中身なし）は断る（SabiRuby の mrblib は 4.1.0-rc2 製で
      中身が違うため、必ず中身が要る）。
   3. オブジェクトを 2 回なめる: 1 回目で全部の器を作り（permanent は名前で引く）、2 回目で
      中身を埋める。
   4. permanent なものへの変更（メソッドの定義・削除、ivar・定数・クラス変数、上位の付け
      替え＝permanent なクラスへの include、大域変数）を当てる。
   5. 入れ子のクラスの名前（`A::B` の文字列）から `outer` を決める。
   6. Hash の項目を入れる（鍵の `hash`／`eql?` はプログラムのメソッドでもよいので、全部が
      そろってから `hash_set` で）。
   7. frozen の印を立てる。
   8. スタックと枠を組み立てる。
3. `Vm::checkpoint_run` で、いちばん上の枠の pc から動かす。

### 写し方

| 像 | SabiRuby |
|---|---|
| 枠（callinfo）: 窓の始まり、pc、`n`、`nk`、可視性の 4 bit、環境（Env か定義先のクラス） | `CallInfo`。`kw` は `nk == 15`（ENTER の後は 0 か 15。それ以外は断る）。いちばん下の枠は `Cci::Skip`（`run_irep` と同じ）、他は `Cci::None`。C から入った枠（`cci != 0`）は断る（書き出し側も書かない）。`method_block` は false |
| 枠の pc | そのまま（いちばん上は次の命令、下は呼び出しの次の命令。SabiRuby の `exec_frames` と同じ意味） |
| 枠の渡されたブロック（`ci->blk`） | 使わない。ブロックは枠のブロックのレジスタにもあり、SabiRuby はそこを読む |
| スタック上の Env（位置で書かれる） | `EnvData { attached: true, base: 位置, ctx: ROOT }`。位置がどれかの枠の始まりで、その枠の環境であることを確かめる |
| 切り離し済みの Env | `values` |
| Proc（strict・orphan・scope、本体の irep、upper、環境） | `ProcData`。環境が Env なら定義先のクラスは Env のクラス欄から |
| クラスの「ivar」（定数・クラス変数・`__classname__`・`__outer__`・`__attached__` が 1 つの表） | 定数は `consts`、`@@x` は `cvars`、`__classname__` は `name`（Symbol ならトップレベル、String のパスなら最後の部分と `outer`）、`__attached__` は `attached`、残りは ivars |
| 取り込みクラス（クラス欄が取り込んだモジュール） | `ClassData { iclass_of }` |
| メソッドの項目: Proc / C 関数の名前 / undef、可視性 | `Method::Ruby` / 名前で引いた SabiRuby のメソッド（Native でも Ruby でもそのまま）/ `Method::Undef`、`vis` |
| `attr_reader`／`attr_writer` の Proc（C 関数 + ivar 名を持つ Env） | `Method::AttrReader(ivar)`／`AttrWriter(ivar)` |
| Hash の既定値（ivar `ifnone` + 印） | 既定値は `HashData::default`、既定の Proc は ivar `__default_proc` |
| Exception のメッセージとバックトレース（文字列の配列） | ivar `mesg`、`@__btstr`（`set_backtrace` と同じ置き場） |
| クラスを持たないオブジェクト（C の Proc、C 関数が作る内部の配列。レジスタに残る） | Proc・Array・String・Hash の素のクラス |
| C のデータ（型名 + バイト列） | 型名ごとの復元の関数（今は `Checkpoint::Box` だけ。値を Data の handle に持つ） |

### mrblib の枠: 中身ごと運ぶ

SabiRuby の mrblib は 4.1.0-rc2 の mrbc で作られていて、書き出し側（4.0 系）の mrblib とは
irep の中身が違う。そのため、`Integer#times` や `Array#each` の中（ブロックの中で止まった）
の枠は、名前で SabiRuby のメソッドに結んでも pc が合わない。

書き出し側は、表に入る mrblib の irep（枠が動かしているもの、実行中に作られた Proc が持つ
もの、その子）を中身ごと書く（irep の種類 2）。枠の Proc が mrblib のメソッドなら、名前と
irep の番号を書く（オブジェクトの種類 15）。SabiRuby は、その irep を本体とする Proc を
新しく作って枠に使う（持ち主のクラスは名前から、strict・scope）。その Proc を `upper` に
持つブロックも同じものを指す。枠が戻ると、それ以降はふつうに SabiRuby のメソッドが呼ばれる。

像の大きさへの影響は、mrblib の枠があるときだけ。試験の像で 59〜254 バイト
（`Integer#times` 1 つで 93 バイト）。

## 名前の対応

SabiRuby は対応の表を持たない。名前を自分のクラスをたどって引く（`src/checkpoint/names.rs`）:
`T` は main、`Integer`・`Math::DomainError` は Object からの定数の道筋、`S:x` は x の特異
クラス（無ければ作る）、`I:x/k` は x の直上 k 番目の取り込みクラス、`M:x#m`・`F:x#m` は x の
メソッド表（無ければ祖先から）、`E:x#m` はその Proc の Env、`V:x@v` はクラスの ivar・定数・
クラス変数の値（`@__classname__` は名前の文字列を作る）、`G:$v` は大域変数。`X:`（書き出し側が
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
    枠を遡ってその Proc を探す。SabiRuby の `catch` は native のループ枠（`ext_catch.rs`）で、
    `throw` はそれを探す。運ばれた `catch` の枠は SabiRuby の `throw` に見つからない。
    → その枠を含む像は断る（"a frame runs Kernel#catch"）。
  - **Method オブジェクト**（`3.method(:+)` など）: 書き出し側は中に C 関数の Proc を持つ。
    SabiRuby の Rust のメソッドは Proc ではない。→ 断る（"a Proc made from the C function …"）。

## 試験

`tests/checkpoint.rs`（feature on のときだけ）。`tests/checkpoint/` に、書き出し側の試験の
サンプル（`.rb`）と、それを C mruby で動かして書いた像（`.ckpt`。2 回以上 suspend するものは
鎖の各点の像、32 bit の書き手のものも 1 つずつ）と、期待する出力（`.out`。書き出し側の基準の
実行のその点より後）か断りの文言（`.refused`）を置いた。

| 試験 | 確かめること |
|---|---|
| `images_resume` | 全部の像（29 個）が、`.out` と同じ出力で最後まで動くか、`.refused` の理由で断られる |
| `refused_leaves_vm_idle` | 断られた後の VM で別の像が動く |
| `truncated_images_are_refused` | 像をどこで切っても、panic せずに断る |
| `changed_code_is_refused` | 運ばれたコードを 1 バイト変えると、ハッシュの照合で断る |
| `image_is_the_same_from_32_bit` | 64 bit と 32 bit の書き手の像の違いは 1 バイト |

結果: feature off・on の両方で `cargo test --workspace` が通る。`SABIRUBY_GC_STRESS=1` でも
`tests/checkpoint.rs` は通る。`tools/check_no_std.sh` は feature 無しでも `--features checkpoint`
でも通る。

像の内訳（サンプルごと）:

| サンプル | 中身 | SabiRuby |
|---|---|---|
| counter | while の途中、2 回 suspend | 2 点とも続く |
| block | `times`・`each_with_index`・入れ子の `each` のブロックの中（mrblib の枠） | 3 点とも続く |
| method_middle | メソッド 3 段の途中、省略可能・残り・キーワード引数 | 続く |
| closure | 外の変数を書き換えるクロージャ（スタック上と切り離し済みの Env） | 2 点とも続く |
| user_class | attr_accessor、継承、include（自作と Comparable）、クラスメソッド、クラス変数、定数、特異メソッド | 続く |
| builtin_patch | String・Integer へのメソッド追加、Array の C メソッドの別名、トップレベルの def、main の ivar、大域変数 | 続く |
| hash | 色々な鍵（`hash`／`eql?` を持つ自作の鍵）、既定値、既定の Proc、Range | 続く |
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
| `Kernel#catch` の枠 | `throw` が見つけられない |
| 型名の分からない C のデータ | 復元の関数が無い（今は `Checkpoint::Box` だけ） |
| ENTER の前のキーワード引数（`nk` が 1〜14） | SabiRuby の枠はキーワードを Hash 1 つで持つ |
| C から入った枠 | 書き出し側も書かない（形式の約束） |

書き出し側が断るもの（Fiber・Task の中、Struct・Set などの型、prepend、C の関数が呼んだ
ブロックの中）は像にならない。

## 既知の限り（この段では直していない）

- **古いレジスタ**: 枠の窓の中に、もう使われない値が残っていることがある。書き出し側が
  第 2 段で命令列から生きているレジスタを求め、死んだ一時レジスタを nil で書くようになった
  （下の「第 2 段」）。読む側は何もしない。
- 枠の Proc（mrblib のメソッド）は、新しく作った Proc が代わりに立つ。`Method#==` などで
  その Proc の同一性を見るプログラムは違いが出うる。
- 断ったときの VM: 変更表を当てた後で断った場合（Hash の鍵の `hash` が例外を出した、枠の
  検査で断った）は、permanent なクラスへの変更が残る。ホストはその VM を捨てる前提。
  切れた像・版の違いなどは読む段で断るので、何も変わらない。
- 行番号: 第 2 段で形式に任意の節 `DBG0` が入り、運んだ irep に付けるようにした。
- 像は `.mrb` と同じく信頼する入力として扱う。ハッシュの照合は事故の検出で、改ざんの防止
  ではない。命令列の中身の検査はしていない（`load` と同じ）。

## 第 2 段: 形式の追補に合わせる（2026-11-03）

書き出し側が、外から止めた試験一式（mruby 付属の試験をランダムな命令の境目で止める）を
運ぶために形式へ足したものに、この読み手を合わせた。

| 追補 | この読み手 |
|---|---|
| VM の後の任意の節（`DBG0` 行番号、`GCS0` GC の設定）と終わりの印 `END0` | `DBG0` は運んだ irep の `lines`・`filename` に当てる。`GCS0` は読み飛ばす（GC の作りが違う）。`END0` が無ければ切れた像として断る |
| 本体の種類 2 の Proc（`alias_method` の別名） | メソッド表の項目としては元のメソッド（`upper`）に結ぶ。値として要るときは断る |
| 種類 25 の String（`String#b` の印） | `binary` を立てた String |
| C のデータ `Time` | 秒・ナノ秒・時間帯から、この VM の Time（隠れた ivar）を作る |
| 変更表の種類 10（permanent な String・Array の中身。`ARGV << x`） | 中身を入れ替える。Hash は断る |
| 子の数があって子の表が無い irep（書き手の mrblib の一部） | 子を結ばない |
| 種類 16〜24（bigint・Struct・Set・Rational・Complex・ensure を通る break・Random・prepend の origin）、変更表の種類 8・9 | まだ運ばない。何が無いかを添えて断る |

CLI は `-r lib` を `--resume` のときも先に読む（ホストの準備。試験一式を続けるときは、書き
出し側の試験の環境 `t_print` などを Ruby で定義したものを渡す）。

試験の像（`tests/checkpoint/`）は、この追補を含む書き出し側で作り直した（中身の試験は
同じ）。

書き出し側の試験一式の一部（中核の試験 37 本、1 本 20 点）を、ここで続けたもの: 740 点
のうち書き出し 729、ここで断ったもの 13（prepend の origin 12、ensure を通る break 1）、
続いた 716 はすべて往復なしの出力と一致した。中核の試験の
うち 6 本は、像なしで最初から動かしてもこの VM と書き出し側の出力が違うので外した:
`codegen.rb`（`class Regexp; end` の後の正規表現リテラル）、`gc.rb`（`GC.malloc_threshold=`）、
`string.rb`（`String#index` の UTF-8）、`vformat.rb`（書き出し側の C の試験関数）と、比べ方の
都合の 2 本（`range.rb`・`syntax.rb`: プログラムのファイル名が両者で違い、バックトレースと
`__FILE__` に出る）。

## 残り（逆向き: SabiRuby → C mruby で要りそうなこと）

- 書き出す場所: SabiRuby には C mruby の `task.switching` に当たる安全点がすでにある
  （`exec_frames` の頭の 4 つの判定）。`Checkpoint.save` の印をそこに相乗りさせる形が自然。
- 名前を付ける側: 逆向きでは SabiRuby が自分の組み込みに C mruby と同じ名前を付ける。この段の
  「名前を引く」手順の逆（クラスの表をなめて名前を付ける）が要る。
- SabiRuby の native ループ枠（`push_loop_frame`。`catch`、`sort { }`、`Array.new(n) { }` など）は
  C mruby では C の関数の枠で、運べない。逆向きでは断る場面になる。
- `Method::Closure`・Data（handle）・HostStore の値は C mruby に対応が無い。
- Value の `Slot` と `Env` の持ち方は形式とそのまま対応する（この段で確かめた）。

## 作者に決めてほしいこと

1. feature の名前と置き場（`checkpoint`、VM の crate の中）でよいか。別の crate にする手も
   ある（VM の内部の型を使うので、今は中に置いた）。
2. `Checkpoint` モジュールを SabiRuby 側でどこまで持つか（今は resume のときだけ定義し、
   書き出しはしない）。
3. 試験の像（`tests/checkpoint/*.ckpt`、約 55 KB）を置くことと、それを作る書き出し側が
   このリポジトリの外にあること。

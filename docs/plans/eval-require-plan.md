# eval と require の検討メモ

作成 2026-09-12。実装指示書ではなく、検討用（著者「evalができるとRubyらしくなるし、requireとかも実装できるようになるよね？ 検討用のドキュメントを残してほしい」）。

> **実装状況**（2026-09-12）: 4.4 の段階 1・2（eval）と 4（binding、`Proc#binding`）は実装済み。
> ここに書いた設計のとおり（`Host` の差し込み口 `src/host.rs`、vendoring したコンパイラへの
> パッチ `SABIRUBY_EVAL_SCOPES`、`src/builtins/ext_eval.rs`／`ext_binding.rs`）。
> 本家テストは `gem_eval` 18/18、`gem_binding` 8/8、mruby-binding／mruby-proc-binding 自身の
> テストは C 補助コードの 3 件を除いて通る。段階 3（require／load、下記 5 節）は
> 2026-09-13 に実装した（`37d4a31`、5 節の見出しの「済み」）。この行は書いた時点（2026-09-12）の
> 「未着手」のまま残っていたので、0.7.0 の片付け（`release-0.7-plan.md` の S7）で直した。
> 実装で変わった点: `Binding#eval` の `expand_lvspace` は「文字列を一度コンパイルして、その
> irep の `lv`（＝外側で解決しなかった名前）を binding に足し、もう一度コンパイルする」形にした
> （パーサを 2 回呼ぶ代わりにコンパイラを 2 回呼ぶ。`Host` に構文解析だけの入口を足さずに済む）。

根拠にしたソース:

* 本家 mruby 4.1.0-rc（`../../ref/mruby`、`3cf73ee`）: `mrbgems/mruby-eval/src/eval.c`、`mrbgems/mruby-binding/src/binding.c`、
  `mrbgems/mruby-compiler/src/{codegen,compile}.c`、`src/vm.c`（`uvenv`）。
* PicoRuby（`../../ref/picoruby`、`80efbea3`、2026-09-11）: `mrbgems/picoruby-eval`、`picoruby-require`、`picoruby-sandbox`、
  `picoruby-mruby/src/mrc_utils.c`、同梱 `mruby-compiler`（本家より新しい版）。
* SabiRuby: `src/vm.rs`（`uvenv`、`frame_env`、`run_irep`、`load`）、`src/builtins/ext_metaprog.rs`（`local_variables`）、
  `compiler/csrc/shim.c`、`cli/src/main.rs`。

## 0. 結論

* **eval は実装できる**。必要なのは (a) コンパイラ crate に「外側のローカル変数名の表」を受け取る入口を 1 つ足すこと（vendoring した C への小さなパッチ）、
  (b) VM に「コンパイラの差し込み口」を 1 つ足すこと。VM crate はコンパイラを知らないまま（no_std、純 Rust のまま）でよい。
* **require も実装できる**。eval と同じ差し込み口に「ファイルを読む」関数を足すだけで、Ruby 側のロジックは PicoRuby の `require.rb` がほぼそのまま使える。
  ファイルの読み方はホスト（CLI、Playground、rubevy）ごとに違うので、VM には持たせない。
* 順序は eval → require → binding。工数はそれぞれ 1〜2 日、半日〜1 日、1〜2 日。
* eval が入っても、既存の経路（`sabiruby compile` の出力が本家 mrbc と 1 バイトも違わない）は変わらない。eval 用の入口は別関数にする。

## 1. 本家の eval の仕組み（`mruby-eval`）

`Kernel#eval(str, binding=nil, file=nil, line=1)`、`BasicObject#instance_eval(str)`、`Module#class_eval(str)`／`module_eval`、`Binding#eval`。
すべて `create_proc_from_string` を通る。

1. **コンパイル文脈に呼び出し元の Proc を渡す**。`cc->upper = 呼び出し元フレームの proc`（binding があれば binding の proc）。
   `capture_errors`、`no_optimize = TRUE`、ファイル名は既定で `(eval)`、行番号は引数の `line`。
2. **パーサ（Prism）に外側のローカル変数名を教える**（`compile.c` の `mrc_pm_options_init`、`MRC_TARGET_MRUBY` のときだけ）。
   `cc->upper` から `upper` を辿り、各 Proc の irep の `lv`（ローカル変数名表）を Prism の `pm_options` の scopes に積む。
   これで Prism は `eval "a"` の `a` をメソッド呼び出しではなくローカル変数の参照（`PM_LOCAL_VARIABLE_READ_NODE`、depth 付き）として解析する。
3. **コード生成で外側の変数を upvar に落とす**（`codegen.c` の `search_upvar`、`MRC_TARGET_MRUBY` のときだけ）。
   文字列の中のスコープに無い名前は、`s->c->upper` の Proc 連鎖を辿って irep の `lv` を引き、`GETUPVAR`／`SETUPVAR` の（番号、段数）を決める。
   段数は「eval 文字列の最上位スコープを 0 として、呼び出し元の Proc が 0」になるよう `lv - 1` で返す（下記 4 の理由）。
   番号付きパラメータ `_1`〜`_9` も同じ仕組みで外側を引く（`mrc_mruby_numbered_parameter_upvar`）。
4. **出来た Proc に呼び出し元の環境を付ける**。呼び出し元フレームの `REnv`（無ければ `mrb_env_new` でその場で作り、フレームに登録）を
   新しい Proc の `env` にし、`upper = 呼び出し元 proc`、ターゲットクラスも呼び出し元のものにする。
   VM の `uvenv(up)` は「現在の proc から `upper` を `up` 回辿った proc の env」を返すので、
   eval の Proc 自身の env が呼び出し元の env になっていれば、段数 0 の upvar がそのまま呼び出し元の変数になる。
   環境が「フレームに付いたまま」（`attached`）なら、`eval "a = 20"` は呼び出し元のレジスタに直接書く。
5. `mrb_exec_irep` で呼び出し元の `self` で実行。引数無し、ブロック無し、可視性は既定に戻す。

binding 付きの場合は、さらに `binding_eval_prepare` が eval 文字列を一度パースして新しく定義されるローカル変数名を集め、
binding 側の変数空間（`lvspace` proc）を広げてから本コンパイルする（`expand_lvspace`）。これが「binding で eval した変数が次の eval でも見える」仕組み。

### 本家 4.1.0-rc と PicoRuby 同梱版の差

本家 4.1.0-rc の `search_upvar` は `MRC_PROC_SCOPE_P` の proc（メソッド本体など）を調べたところで止まる。ただし `mrc_pm_options_init` の方は C 関数の proc まで辿るので、
パーサに教える名前の範囲とコード生成が引ける範囲が一致していない（外側のメソッドの変数を Prism はローカル変数と解析し、codegen が「Can't find local variables」にする）。
PicoRuby 同梱の `mruby-compiler` は本家より新しく、両方の走査が `MRC_PROC_LVAR_BOUNDARY_P`（`SCOPE` かつ env 無しの proc）で止まるように揃えてある。
SabiRuby の `local_variables` は `pd.scope` で止めており、これと同じ。eval の名前表も `scope` で止める（メソッド本体の外の変数は見えないのが Ruby の仕様）。

## 2. PicoRuby の手法

PicoRuby は VM を 2 つ（mruby、mruby/c）持ち、`#if defined(PICORB_VM_MRUBY)` で切り替える。eval と require の実装はそれぞれ別。

### 2.1 eval

* **mruby 側**（`picoruby-eval/src/mruby/eval.c`）: 本家 `mruby-eval` の `create_proc_from_string` をほぼそのまま持ち込んだもの。
  `cc->upper = scope`、`no_optimize`、`mrb_vm_ci_env` か `mrb_env_new` で環境を付け、`mrb_exec_irep`。
  違いは `file`／`line` 引数を取らないこと、`instance_eval`／`class_eval` の文字列形式が無いこと、`capture_errors` が `FALSE`（TODO）なこと、
  コンパイル直後に `mrc_resolve_intern`（`picoruby-mruby/src/mrc_utils.c`。PicoRuby 独自で、本家 4.1.0-rc には無い）で
  irep の中に残った Prism の定数プール ID を mruby のシンボルに引き直すこと（本家は `MRC_TARGET_MRUBY` のときコード生成中に `mrb_intern` する）。
  SabiRuby はバイトコード（RITE）を受け取って `Vm::load` でシンボルを intern するので、この段は要らない。
  つまり **PicoRuby の mruby 側 eval は、コンパイラを `MRC_TARGET_MRUBY` で VM と一緒にリンクし、コンパイラが VM の `RProc` を直接読む**本家の方式。
  SabiRuby はこの方式を取れない（下記 3）。
* **mruby/c 側**（`picoruby-eval/src/mrubyc/eval.c`）: 文字列を `mrc_ccontext_new(NULL)`（VM 無し）でコンパイルし、`mrc_dump_irep` で
  RITE バイナリにして、**新しいタスクとして走らせる**。外側のローカル変数は見えず、戻り値も nil（`test/eval_test.rb` は mruby/c のとき `nil` を期待する）。
  これは「VM から独立したコンパイラを使って、できる範囲の eval を付ける」方式。SabiRuby の最小案（下記 4.4 の段階 1）と同じ。
* **IRB の継続**（`picoruby-sandbox/src/mruby/sandbox.c` の `Sandbox#result`、mruby/c 側は `compile.c` の `PICORB_VM_MRUBYC` 部分）:
  直前にコンパイルした irep の `lv` から Prism の scopes を 1 段作り、次のコンパイルに渡す。これで IRB の前の行で定義した変数が次の行で「ローカル変数」と解析される。
  ただし **値の受け渡しは別**で、Prism に名前を教えるだけでは前の行の値は見えない。PicoRuby の IRB（Sandbox）は前回の irep のレジスタを保つことで実現している
  （`scope_sp` などの仕掛け。詳細は未確認）。SabiRuby の Playground に REPL を付けるときの先行例。

### 2.2 require

* Ruby 側（`picoruby-require/mrblib/require.rb`）。`Kernel#require(name)`:
  1. `$LOADED_FEATURES` にあれば `false`。
  2. `extern(name)` で **組み込み gem** を引く。mruby 側の `extern`（`src/mruby/require.c`）は、ビルド時に生成した `prebuilt_gems[]`
     （`templates/mruby/picogem_init.c.erb`、名前だけの表）に名前があれば `false` を返す。mruby 側では gem はすべて起動時にリンク済みなので、
     `require 'gpio'` は「既に読み込まれている」の意味で `false`。
     mruby/c 側は gem ごとに初期化関数とバイトコードを持ち、`require` されたときに初めて走らせる（遅延ロード。`src/mrubyc/require.c`）。
  3. どちらでもなければ `require_file`: `$LOAD_PATH` の各ディレクトリで `name.mrb` → `name.rb` の順に `File.file?` を試し、
     見つかったら `load_file`。無ければ `LoadError`。
* `load_file`（`require.rb` → `picoruby-sandbox/mrblib/sandbox.rb` の `Sandbox#load_file`）:
  ファイルを読み、先頭が `RITE_VERSION`（例 `RITE0400`）ならバイトコードとしてそのまま実行、そうでなければ `Sandbox#compile(rb, filename: path)` で
  コンパイルして実行。**新しいタスク**（`mrc_create_task`、`top_self = Object`）で走らせ、終わるまで待つ（`wait`）。例外はタスクの結果から取り出して投げ直す。
  つまり require したファイルは新しい最上位スコープで走る（CRuby と同じ。呼び出し元のローカル変数は見えない）。
* `load(path)` は `$LOADED_FEATURES` を見ずに毎回読む。`$LOADED_FEATURES` の初期値は組み込み gem 名の一覧。
* `RUBY_PLATFORM`、`RUBY_DESCRIPTION`、`PICORUBY_VERSION`、`RITE_VERSION` の定数はこの gem が定義する。

### 2.3 SabiRuby から見た PicoRuby の要点

* コンパイラと VM の結合は `MRC_TARGET_MRUBY` の C マクロ 1 つで、結合部は `search_upvar`、`mrc_pm_options_init`、
  `mrc_mruby_numbered_parameter_upvar` の 3 か所と、環境を付ける eval 側の数行。**結合面は狭い**。
  SabiRuby はこの 3 か所に「Proc 連鎖の代わりに名前表を引く」分岐を足せばよい。
* require は VM の機能ではなく、「ファイルを読む」「コンパイルする」「新しい最上位スコープで走らせる」の 3 つを Ruby で束ねたもの。
  SabiRuby も同じ分け方にし、前 2 つをホストに、最後を VM に置く。
* PicoRuby は require したファイルを別タスクで走らせる。SabiRuby にはタスクが無いが、必要なのは「新しい最上位スコープ」であって「別の実行単位」ではない。
  `run_irep` と同じ形の入れ子の `run_loop` で足りる（eval と同じ）。Fiber の中から require しても、入れ子ループは現在の Context 上で回るので問題ない
  （ホスト境界の制約 `docs/design/fibers.md` は「ネイティブ関数を挟む切り替え」の話で、require の途中で `Fiber.yield` する使い方は本家でも例外になる）。

## 3. SabiRuby の制約

1. `sabiruby-compiler` は本家 `mrbc` と同じ standalone 構成（`MRC_TARGET_MRUBY` 無し）。
   `search_upvar` の Proc 連鎖の分岐が無く、外側の変数を参照すると「Can't find local variables」でコンパイルエラーになる。
   `MRC_TARGET_MRUBY` を付けても、コンパイラが読むのは本家の `struct RProc`／`mrb_irep` で、SabiRuby の `ProcData` ではない。
2. VM crate（`sabiruby`）は no_std + alloc の純 Rust で、C をリンクする `sabiruby-compiler`（std、`cc`）に依存させない（README の Rules）。
   `Kernel#eval` は VM のメソッドなので、そのままでは VM がコンパイラを呼ぶ形になる。
3. VM にファイル I/O が無い（no_std）。require のファイル読み込みはホストの仕事。
4. コンパイラはグローバルロック（`mrc_presym` の static）を持つ。eval の途中で別スレッドがコンパイルすることは CLI では無いが、
   Playground（Worker 1 本）でも同じく無い。再入（コンパイル中にコンパイル）は起きない（コンパイルは Ruby を実行しない）。

## 4. 設計

### 4.1 依存の向き: VM に「ホスト」の差し込み口を置く

```rust
// sabiruby (VM crate)  src/host.rs
pub struct EvalOptions<'a> {
    pub filename: &'a str,          // 既定 "(eval)"、require ではファイルパス
    pub line: u32,                  // eval の line 引数
    pub scopes: &'a [Vec<Vec<u8>>], // 外側のローカル変数名。scopes[0] が呼び出し元、後ろへ行くほど外側。空なら最上位スクリプトとして扱う
    pub debug_info: bool,
}
pub trait Host {
    /// Ruby ソース → RITE バイナリ。Err はコンパイラの診断（SyntaxError の文言に使う）
    fn compile(&mut self, src: &[u8], opts: &EvalOptions) -> Result<Vec<u8>, String>;
    /// require/load のファイル読み込み。None なら「無い」
    fn read_file(&mut self, path: &str) -> Option<Vec<u8>> { let _ = path; None }
    /// File.file? 相当（require_file の探索用）。既定は read_file で代用
    fn file_exists(&mut self, path: &str) -> bool { self.read_file(path).is_some() }
}
impl Vm { pub fn set_host(&mut self, host: Box<dyn Host>) }
```

* `Box<dyn Host>` は alloc だけで使えるので no_std を壊さない。
* ホスト未登録のとき、`eval` は `NotImplementedError`（`eval: no compiler installed`）、`require` は `LoadError` を投げる。
  gem を移植しない構成（rubevy の既定など）ではそもそも `eval` を定義しない選択もできる（feature `eval`）。
* `sabiruby-compiler` 側に `impl sabiruby::Host for Compiler` を feature `host`（`dep:sabiruby`）で用意する。
  依存の向きは compiler → sabiruby で、sabiruby は compiler を知らない。CLI、Playground、rubevy は feature を有効にして `vm.set_host(Box::new(Compiler::new()))` するだけ。
  `read_file` は CLI では `std::fs`、Playground では見本ファイルの仮想 FS（`.rb` を名前で引く表）、rubevy では Bevy のアセットから。
* Playground の C ABI（`docs/design/playground.md`）には `sabi_compile` が既にある。eval 用は Worker 内で同じ wasm モジュールの中から呼ぶので、
  JS を経由しない。追加は C ABI ではなく Rust の `Host` 実装 1 つ。

### 4.2 コンパイラ側の差し込み口（vendoring した C への唯一の改変）

`compiler/csrc/shim.c` に新しい入口を足す。既存の `sabiruby_mrc_compile` は触らない（黄金テストが守っているもの）。

```c
/* scopes: 外側のスコープごとの名前の並び。names[i] は NUL 区切り連結、counts[i] は個数。scopes[0] が呼び出し元 */
int sabiruby_mrc_compile_eval(const uint8_t *src, size_t len, const char *filename, uint32_t line, unsigned flags,
                              const char *const *names, const uint32_t *counts, size_t nscopes,
                              uint8_t **out, size_t *out_len, char **diag);
```

vendoring した `mruby-compiler` に `SABIRUBY_EVAL_SCOPES` の分岐を足す（`MRC_TARGET_MRUBY` の分岐と並べる。`#elif`）。

1. `mrc_ccontext` に `const struct sabiruby_scopes *eval_scopes;` を足す（`MRC_TARGET_MRUBY` の `upper` と同じ場所）。
2. `compile.c` `mrc_pm_options_init`: `eval_scopes` から Prism の scopes を作る（本家が Proc 連鎖から作っているのと同じ形。深い方が先、呼び出し元が最後、その後ろに Prism が要求する空の 1 段）。
3. `codegen.c` `search_upvar`: コード生成スコープを使い切ったら、`eval_scopes` を呼び出し元側から順に引き、見つかったら `*idx = 番号(1 始まり)`、戻り値は `lv - 1`（本家と同じ計算）。
   `lv_idx` と同じく、名前の並びは irep の `lv` の順（`nlocals - 1` 個。`*`／`&` の無名パラメータの穴は空文字列で埋める）で渡し、番号を狂わせない。
4. `codegen.c` `mrc_mruby_numbered_parameter_upvar`: 同じ表を引く（`_1` は `lv` に普通の名前として入っている）。
5. `no_optimize` を立てる（本家の eval が立てている。理由はソースに書かれていないので未確認。同じにする）。

パッチは 3 ファイル、50 行前後。`compiler/vendor/VENDOR.md` に「本家からの唯一の差分」として diff を残し、本家を更新するときに当て直せるようにする。
`sabiruby_mrc_compile`（scopes 無し）の出力が本家と同じままであることは、既存の黄金テスト 85 本がそのまま検証する。

代案（採らない）: パッチ無しで、eval 文字列の前に `a = a` のような前置きを付けて変数を「定義済み」に見せる方法。
値が来ないので読み書きが呼び出し元に届かず、eval の意味を成さない。PicoRuby の mruby/c 側（外側が見えない eval）と同じ範囲にしかならない。

### 4.3 VM 側（`src/builtins/ext_eval.rs`）

`Kernel#eval(str, binding=nil, file=nil, line=1)`、`BasicObject#instance_eval(str, file, line)`、`Module#class_eval(str, file, line)`／`module_eval`。
ブロック形式の `instance_eval`／`class_eval`／`module_eval` は既にある（`src/builtins/object.rs`）ので、引数が文字列のときだけ新しい経路に入る。

1. **名前表を作る**。呼び出し元フレームの proc から `upper` を辿り、各 irep の `lv` を `Vec<Vec<u8>>` にする。
   `local_variables`（`ext_metaprog.rs`）と同じ歩き方（`pd.scope` で止める）。`*`／`&` の穴は空文字列。
   `instance_eval`／`class_eval` の文字列形式も、本家（`object_eval` → `create_proc_from_string`）と同じく呼び出し元の変数が見える。
2. **コンパイル**。`host.compile(src, &EvalOptions{ filename, line, scopes, debug_info: true })`。
   Err なら `SyntaxError`（文言は本家 `capture_errors` の形式 `file:line:col: message` に合わせる。診断の整形は `sabiruby-compiler` の `Diagnostic` にある）。
3. **ロード**。`Vm::load(bin)` で irep 群を `ireps` に足す（既存）。irep は消さない（Proc が参照する。本家も eval の irep は Proc と共に生きる）。
4. **Proc を作る**。`ProcData { irep, upper: Some(呼び出し元 proc), env: Some(呼び出し元フレームの env), target_class: 呼び出し元の, strict: false, scope: false, orphan: false }`。
   env は `frame_env()` で「無ければ作ってフレームに付ける」（本家の `mrb_vm_ci_env` ／ `mrb_env_new` と同じ。既に `frame_env` がこの意味）。
   ここで `attached: true` の env が呼び出し元のレジスタを指すので、`eval "a = 20"` が呼び出し元に書ける。
   `scope: false` にするのは、eval 文字列の中の `return` が呼び出し元のメソッドから返るため（本家は ENVSET のみで SCOPE を立てない）。
5. **実行**。`call_proc(proc, self_, &[], nil, mid, target_class)` 相当で入れ子の `run_loop`。`self` は eval なら呼び出し元の `self`、
   `instance_eval` なら receiver（ターゲットクラスは特異クラス）、`class_eval` なら receiver（ターゲットクラスは receiver）。
   引数 0、ブロック無し、可視性は既定（本家 `eval_irep`：`ci->n = 0`、`MRB_CI_SET_VISIBILITY_BREAK`）。
6. `eval` の中で定義した **新しいローカル変数**は eval の Proc の自分のレジスタに置かれ、呼び出し元には見えない（本家も同じ。binding 経由だけ広がる）。

Fiber との関係: eval の Proc の env は `ctx: self.cur` を持つ。eval の中で `Fiber.new { a }` のように外側の変数を閉じ込めたブロックを作ると、
env は呼び出し元フレームのもので、フレームが返れば `attached: false` に外れる。既存の仕組みで足りる（`docs/design/fibers.md` の env の扱い）。

GC: `native_active > 0` の間は回収しない約束（`docs/design/gc.md`）があるので、`eval` ネイティブの中で作った Proc と env はホストのコンパイル中も安全。
`load` が `ireps` に足した irep は GC 対象外（irep は `Vec<VmIrep>` で回収しない。eval を大量に繰り返すと irep が溜まる。本家は Proc の GC と共に irep を解放する。
「irep の回収」は将来課題として `docs/design/gc.md` に書く）。

### 4.4 段階

1. **eval（外側の変数なし）**: ホスト差し込み口と `Kernel#eval` だけ。コンパイラのパッチ無し。PicoRuby の mruby/c 側と同じ範囲。
   `mruby-eval/test/eval.rb` の 18 件のうち、外側の変数を使わないものが通る。半日。ここまでで Playground に「eval が動く」が出せる。
2. **eval（外側の変数あり）**: コンパイラのパッチ、名前表、`instance_eval`／`class_eval` の文字列形式。`eval.rb` を全部通す。1 日。
3. **require／load**: 下記 5。半日〜1 日。
4. **binding**: `mruby-binding`（C 523 行、テスト 102 行）。`Kernel#binding` が呼び出し元の proc と env を包む `Binding` を作る。
   `local_variable_get/set/defined?`、`local_variables`、`receiver`、`source_location`、`Binding#eval`。
   `expand_lvspace`（eval で増えた変数を binding に足す）は、SabiRuby では「binding が持つ env の `values` を伸ばし、名前表を binding 側で持つ」形になる。1〜2 日。

## 5. require／load の設計 — **済み（2026-09-13）**

実装は `src/mrblib/require.rb`（Ruby 側）と `src/builtins/ext_require.rs`（3 つのネイティブ）、
`Vm::set_load_path`、CLI の `-r` と `$LOAD_PATH`。テストは `tests/require.rs`（10 件、期待値は CRuby 3.2 で実測）と
`cli/tests/cli.rs` の 2 件。設計との違いは 2 つだけ:

* 拡張子を持つ名前（`require "./x.rb"`）は付け足さずにそのまま探す（CRuby と同じ）。
* 例外が出たら `$LOADED_FEATURES` から取り消す（CRuby と同じ）。再送は `raise e`（`$!` が無いため裸の `raise` は使えない）。

Ruby 側は PicoRuby の `require.rb` を土台にする（MIT。`$LOADED_FEATURES`、`$LOAD_PATH`、`require_file` の探索順 `.mrb` → `.rb`、`LoadError` の文言）。
SabiRuby 向けの違い:

* `extern` は要らない。組み込み gem は起動時に全部読み込み済みなので、`$LOADED_FEATURES` の初期値に gem 名（`fiber`、`enumerator`、`array-ext`、… `docs/design/gems.md` の順）を入れ、
  `require 'fiber'` が `false` を返すようにする（PicoRuby の mruby 側と同じ振る舞い）。
* `File.file?`／`File.expand_path` は SabiRuby に無い（POSIX 系 gem は対象外）。代わりにネイティブ `__load_file(path) -> String | nil`（ホストの `read_file`）と
  `__file_exist?(path)`（ホストの `file_exists`）を Kernel の private に置き、パスの連結は Ruby 側で文字列として行う（`"#{dir}/#{name}.rb"`。`expand_path` の正規化は省く）。
* `load_file` は Sandbox ではなく、ネイティブ `__exec_file(bytes, path)`:
  1. 先頭が `RITE` ならバイトコード。バージョンが `0400` でなければ `LoadError`（`invalid RITE version`）。
  2. そうでなければ `host.compile(bytes, &EvalOptions{ filename: path, scopes: &[], .. })`。エラーは `SyntaxError`。
  3. `Vm::load` → `run_irep` と同じ形の Proc（`upper: None`、`env: None`、`target_class: Object`、`scope: true`）を作り、`self = top_self` で入れ子の `run_loop`。
     戻り値は捨て、`require` は `true` を返す。例外はそのまま伝播（PicoRuby がタスクの結果から投げ直しているのと同じ効果）。
* `$LOAD_PATH` の初期値はホストが決める（CLI: スクリプトのディレクトリとカレント。Playground: `[""]`。rubevy: アセットのルート）。
  `Vm::set_load_path(&[&str])` を用意する。
* 循環 require は `$LOADED_FEATURES` に読み込み開始時点で入れて防ぐ（CRuby と同じ。PicoRuby の `load_file` は実行が終わってから入れるので、A が B を、B が A を require すると止まらない。動かして確かめてはいない）。

テスト: 本家 mruby に require は無く、PicoRuby の `picoruby-require` にも `test/` は無い。
`tests/fixtures/require_*.rb` を自前で書き、CLI 経由で（一時ディレクトリに `.rb` と `.mrb` を置いて）確認する。
本家の出力との照合はできないので、CRuby（`ruby`）の出力と照合する（`require` の戻り値、`$LOADED_FEATURES` の増え方、`LoadError` の文言）。

## 6. 検証

* `tests/custom/`（SabiRuby 独自のテスト。2026-09-12 に枠を作った）に eval と binding の 4 件を `# pending: eval`／`# pending: binding` で先に置いてある。
  eval が入ったら `pending` を外す。期待値は本家 rc ではなく修正後の挙動（`eval_class_eval_scope`: master `300cc9532`、`eval_outer_scope`: CRuby）。
* `sabiruby compile` の黄金テスト 85 本が変わらないこと（コンパイラのパッチが既存経路に影響しない証拠）。
* `mruby-eval/test/eval.rb`（18 件）を `tools/mrbtest.sh` の `GEMS` に足して全件通す。`mrbtest` の実行器は CLI 側にあり、ホストを登録できる。
  `Kernel#eval` のテストは `a = 10; eval "a"`、`lambda { a = 10; eval "c = a + c" }.call`（2 段外側への書き込み）、`eval 'lambda { c }.call'`（eval の中で作ったブロックからさらに外側）、
  `eval 'def f(a); b=a+1; end'`（メソッド定義）を含む。段階 2 の完成条件。
* `mruby-binding/test/binding.rb` を段階 4 で足す。
* Playground で `eval` と `require`（見本ファイル間）が動くこと。wasm ビルド（`wasm32-wasip1`）でパッチ済みコンパイラが通ること。
* no_std チェック（`tools/check_no_std.sh`）と thumbv7em／wasm32 ビルドが通ること（`Host` trait は alloc だけ）。

## 7. リスクと未確認

* コンパイラのパッチが本家追随の負担になる。結合面は 3 か所で、本家側も同じ場所に `MRC_TARGET_MRUBY` の分岐を持ち続けているので、当て直しは機械的にできる見込み。
* `no_optimize` の理由（未確認）。本家の eval が立てているので同じにするが、peephole が upvar と絡む箇所があるなら、パッチ側で `no_optimize` 無しでも正しいことを確かめてから外せる。
* Prism の scopes に渡す名前と codegen の名前表の**並びの一致**。Prism は名前で depth を決め、codegen は同じ表で番号を決めるので、両方に同じ表を渡せばずれない。
  本家も `mrc_pm_options_init` と `search_upvar` が同じ Proc 連鎖を別々に歩いている（`lvspace` proc の扱いだけ注意）。
* eval の irep が回収されない（4.3 の GC の項）。REPL 用途では気になるので、Playground の REPL を作るときに「irep を Proc から参照カウントする」か「GC の根から irep を辿る」を決める。
* PicoRuby の IRB が前の行の**値**をどう持ち越しているか（`scope_sp`、Sandbox の再実行）は未確認。REPL を作るときに読む。
* `Binding#eval` の `expand_lvspace` を SabiRuby の env（`values: Vec`）でどう表すかは段階 4 で決める。

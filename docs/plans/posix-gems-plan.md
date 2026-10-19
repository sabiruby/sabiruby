# POSIX の gem を、取り外せる別の crate として足す計画

作成 2026-10-19（案）。著者「Posix関連のGemは取り外し可能にした方がいいかなと思ってるがどう思う？」「それぞれ計画書書いてみて」（2026-10-19）。
調査は本体が依頼したもの（2026-10-19、読むだけ）。

## 目的

本家の既定の gembox（`default.gembox` → `stdlib-io.gembox`）に入っている POSIX の 7 つの gem を、VM の crate の外の別の crate として足す。

- **7 つの gem**: mruby-io、mruby-socket、mruby-errno、mruby-dir、mruby-env、mruby-signal、mruby-process
- **使い分け**: ホストが要る分だけ VM に登録する。CLI では有効にし、本家の `mruby` コマンドに近づける。
- **保つもの**: VM は no_std のままにする。ブラウザ・rubevy・ゲームには、ファイルやプロセスに触れる機能を一切入れない。

今、本家にあって SabiRuby に無いメソッド 341 個のうち、312 個がこの gem のものである（`docs/verification/coverage.md`）。答えないメソッドは `Kernel#gets`・`open`・`readline`・`readlines` だけ（インスタンスと特異で 8 個）。

## 本家の 7 つ（調査の要約）

| gem | src C | mrblib | test .rb | 中身 |
|---|---:|---:|---:|---|
| errno | 334 | 50 | 57 | `SystemCallError`、`Errno::Exxx`（約 130 クラス） |
| env | 223 | 153 | 252 | `ENV`（Object の特異メソッド、Enumerable） |
| signal | 108 | 0 | 67 | `Signal.signame`／`list` だけ（trap は無い） |
| io | 3868 | 611 | 1469 | `IO`、`File`、`FileTest`、`IOError`／`EOFError`、`STDIN`/`STDOUT`/`STDERR`、`$stdout` など。**`Kernel` の `puts`・`print`・`p`・`gets` などを Ruby で定義し直す** |
| dir | 530 | 124 | 179 | `Dir` |
| process | 1320 | 21 | 1106 | `Process`（pid、kill、waitpid、clock_gettime、times）、`Process::Status`、`$$`。fork・spawn・system は無く、子プロセスは `IO.popen` |
| socket | 1429 | 1128 | 494 | Addrinfo、BasicSocket 以下、TCP・UDP・UNIX、SocketError |

- 必須の依存: socket → io、process → signal と struct。env → enumerator。
- テストだけの依存: io → time・errno、process → io（popen）・errno。
- errno が無くても io などは動く。errno があれば `Errno::Exxx` を投げる（`src/error.c:592-614`）。

## 今の公開 API で足りないもの

調査で分かったこと（外の crate から gem を足すときに要るもの）:

1. **定数を定義する公開 API**: Errno の約 130 クラスや `File::Constants` に要る。CLI は今 `vm.heap.class_mut(..).consts.insert` で内部を直に触っている。
2. **OS の資源を抱えるオブジェクト**
   - `HostStore<T>` で Data に結んだ資源は、GC で回収されたときと VM を捨てたときに drop される（close される）。資源の持ち方そのものはある。
   - ただし `IO.new`／`File.new` は普通のオブジェクトを確保する。本家の mrblib は、確保したオブジェクトに `super` で fd を入れる形である。確保済みのオブジェクトに Data を付ける公開 API が無い。
3. **Data の dup／clone**: VM が TypeError で断る。io のテストは `io.dup` を使う。
4. **mrbtest のランナーにセットアップの口が無い**（`src/mrbtest.rs` の `run_file_prelude` は手順が固定）。
5. **`$LOADED_FEATURES`** に、gem を入れたときに名前を足す処理。

## 決めたこと（原則で決めた。理由つき）

- **別の crate にする。VM の crate の feature にはしない。** VM の no_std と wasm を構造で守るため。依存の向きは posix → VM だけになる。`sabiruby-serde` と同じく `sabiruby = { default-features = false }` に頼り、utf8 や regexp を強制しない。
- **gem ごとに cargo の feature を分ける**（io、socket、errno、dir、env、signal、process）。本家の gem の依存を feature の依存で写す（socket は io を有効にする、など）。登録はホストが呼ぶ関数 1 つにする（serde の `install_json` と同じ形）。
- **`cfg(unix)` で作る。Windows は対象外**（CI の行列は ubuntu と macOS だけ）。CLI は `[target.'cfg(unix)'.dependencies]` で入れ、Windows の CLI は今のまま gem 無しで建つようにする。
- **VM 側に足す API は、上の 1〜5 を公開の形で足す。** 隠し ivar に Data を入れる回避策（Random と同じ形）は使わない。外から見えない約束に頼ると、ホストが同じことをしたいときに真似できないため。公開 API が増えるので CHANGELOG に書く。
- **unsafe は避ける。**
  - std で書けるもの（ファイル、パイプ付きの子プロセス、TCP/UDP/UNIX ソケット、read_dir、symlink、chmod、pread/pwrite）は std で書く。
  - std に無いもの（kill、waitpid、umask、flock、close_on_exec、IO.select の poll、getppid、各種の clock）は、安全な API を持つ `rustix` を使う。どの関数で使うかは段ごとに確かめる。
  - fd の番号から IO を作る所（`IO.new(fd)`、`IO.for_fd`、`IO.sysopen`）は `FromRawFd`（unsafe）を使わない。crate が持つ fd の表（番号 → 所有している fd。0/1/2 は std の stdin/stdout/stderr）で引く。
- **出力の順を保つ。** io の `Kernel#puts` などは `$stdout.write` に行く。そのため、`STDOUT`（fd 1）への書き込みは VM の出力（`vm.write_out`）を通す既定にし、`vm.out` を通る出力と順が入れ替わらないようにする。
  - CLI は今、出力を最後にまとめて書いている。`print "name? "; gets` のような対話のために、読み込みの前と終わりに吐き出す形に変える。
  - 埋め込み先には、fd に直接書く設定も選べるようにする。
- **数は設定にする**（読み込みのバッファの大きさ、listen の backlog など）。既定値は本家の値で、`docs/numbers.md` に出どころを書く。
- **CLI の `$0`**（本家は設定し、SabiRuby は未設定）は、この計画の仕上げで本家に合わせる。

## 段階

| 段 | 中身 | 確かめ |
|---|---|---|
| P0 | VM 側の土台。定数の定義 API、確保済みのオブジェクトに Data を付ける口と Data の dup をホストに任せる口、mrbtest のランナーのセットアップの口、`$LOADED_FEATURES` への追加。crate の骨組み（名前は著者の判断、feature、`cfg(unix)`）、mrblib を `.mrb` にする手順（rubevy の prelude と同じ形）、`tools/mrbtest.sh` の GEMS 分岐 | VM の挙動は変えない（mrbtest 3 ビルド・custom・ベンチの A/B が同じ）。no_std・wasm の段が通る |
| P1 | errno・env・signal（OS の資源をほぼ持たない）。ENV の書き込みの扱いは下の「著者に聞くこと」 | 本家の errno.rb（30）、env.rb（94）、signal.rb（28） |
| P2 | io の中心。IO と fd の表、STDIN/STDOUT/STDERR、`Kernel` の定義し直し、CLI の逐次出力、read/gets/write/seek/sysopen/pipe/dup、MRubyIOTestUtil の写し（一時ファイル、symlink、UNIX ソケット） | 本家の io.rb（popen 以外） |
| P3 | File・FileTest・dir。パス操作、stat、flock、umask、symlink、chmod、Dir、DirTest（mkdtemp と chdir） | file.rb、file_test.rb、dir.rb |
| P4 | `IO.popen` と process。kill、waitpid、Process::Status、clock、times、`$$`、`$?`、ProcessStatusTest／ProcessClockTest の写し | io.rb の popen、process の 3 ファイル |
| P5 | socket。BasicSocket 以下、Addrinfo（名前解決に使う crate はこの段で選ぶ） | socket の 7 ファイル（127.0.0.1／localhost、UNIX） |
| P6 | 仕上げ。CLI で有効にする、`$0`、`tests/mrbtest.rs` の床（posix の分）、`tools/coverage.sh` で POSIX の 312 を数え直す。docs（gems.md の "Remaining gems"、numbers.md、README）。playground・rubevy・games の `cargo tree -i <crate>` が「一致なし」であることを確かめる | mrbtest 3 ビルド、custom、no_std、wasm、下流の cargo tree |

- P1 は P0 の後すぐに進められる。P2 から先は依存の順に並んでいる。
- socket（P5）は一番大きく、使う人も少ない見込み。P4 までで一度区切って公開してもよい。

## 守ること

- VM の crate に `unsafe` を入れない。`no_std + alloc` と `Vm: Send + Sync` を保つ。VM の既定のビルドの挙動は変えない（gem を登録したときだけ変わる）。
- 新しい crate でも `unsafe` は避ける。どうしても要る所は、報告して著者に選択肢を出す。
- mrbtest の今の数（2346／2269／1981）を下げない。posix の gem のテストは別に数える（本家の `mruby` コマンドの答えを正解とする）。
- 本家のテストは一時ファイル、chdir、子プロセス（`cat`、`echo`）、localhost のソケットを使う。テストは一時ディレクトリの中だけで動かし、終わったら消す。
- MSRV 1.85 で建つこと（`std::io::pipe` は 1.87 からなので使わない）。

## 著者の判断（2026-10-19）

1. crate の名前は **`sabiruby-posix`**。
2. CLI では **既定で有効**（本家の `mruby` コマンドに合わせる。Unix だけ）。
3. ENV の書き込みは **crate の中に写しを持つ**（unsafe を使わない）。起動時の環境を写して読み書きし、子プロセスには `Command::env` で渡す。同じプロセスの C のライブラリの `getenv` には見えないことを README に書く。
4. 進めるのは後で（着手は著者の合図を待つ。正規表現の計画との順番もそのときに決める）。

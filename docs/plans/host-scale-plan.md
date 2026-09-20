# sabiruby: ホストが数千本のタスクを載せたときに見えた VM 側の不足 — 実装指示書

作成 2026-09-20。出どころは rubevy の汎用化（rubevy `docs/plans/generalize-plan.md` の R1〜R8）で、各段階の担当が「VM の話」として挙げたもの。
rubevy の側でできることは済ませた（購読の索引、irep の共有、`frame_time` を上限に、`FrameStats`）。**ここに残っているのは VM を直さないと消えないもの**。
数字の出どころ: rubevy `docs/verification/scale.md`（前後の表と再現手順と罠）、rubevy `docs/worklog/2026-09-20-*.md`（行番号つき）。
対象: sabiruby main `944b72b`（0.5.2 + `sabiruby-serde` の `declare`）。行番号は 0.5.2 のもの。着手前に実物と合っているか確かめる。

**この文書だけで着手できるように書いてある。** 読む順は 1 → 2 → 4 の自分の段階 → 3 の該当節 → 5。

---

## 0. はじめの一歩

```bash
cd /home/kishima/book/kishima
git -C sabiruby worktree add ../sabiruby-wt-host-scale -b host-scale main
cd sabiruby-wt-host-scale && cargo test --workspace     # 着手前 237 passed
./tools/check_no_std.sh                                  # no_std OK
```

作法は `/home/kishima/book/.claude/agents/implementer.md`（「Rust の VM（sabiruby）に関して」の節）と `/home/kishima/book/CLAUDE.md`。
**VM の crate は `no_std + alloc`、unsafe は禁止、`Vm` は `Send + Sync`、本家テストの基準（`tests/mrbtest/baseline*.txt`）を下回らない、
gem が Ruby で定義するメソッドをネイティブで置き換えない、生成物は生成器で作る。** 段階ごとに 1 コミット、push しない、main に触らない、過程は `docs/worklog/` に。
性能に触れる変更は `tools/bench_ab.sh`（交互 A/B）で前後を取る。**ベンチは同時に 1 つだけ、P コア固定、前後は必ず交互に 2 巡**
（この機械には同じバイナリが倍遅くなる「遅い状態」がある）。`cargo fmt` を走らせない。`git stash` を使わない。

---

## 1. 何を直すのか（30 秒版）

| # | 何が起きているか | 見えた数字 |
|---|---|---|
| H1 | **待ちタスクの費用が O(N)**。`wake_sleepers`・`wake_queue_waiters` が毎回 `Q_WAITING` を丸ごと `clone()` して歩く。`q_delete` は 4 本のキューを全部歩く。`pending()` は待ちタスクを最後まで歩きうる。締切に気づく粒度も待ちタスク数に比例して粗くなる | 1 台 1 回の費用が 300 台 9.0 µs → 3000 台 16.8 µs（台数の 2 乗の項）。push 1 回が読み手 1000 本の park で 705 ns（park していなければ 92 ns）。問いを出さない 3000 本だけで `frame_time` 1 ms の設定に対し tick 2.93 ms |
| H2 | **irep が二度と返らない**。`Vm::ireps` は `Vec` で、消す箇所が 1 つも無い。スクリプトを差し替えるたびに積み上がる | rubevy の R2 で「同じ本文は同じ irep」にしたので同じ Apply は 0 になったが、**違う本文の Apply は 1 回につきそのプログラムの irep 数ぶん**永久に残る。プレイヤーが Ruby を書き換えるゲームでは上限が無い |
| H3 | **ホストが数えたい数を言う口が無い**。`Vm::ireps` は `#[doc(hidden)] pub` で、rubevy のテストと計測器がそれに寄りかかっている。ready のまま残ったタスク数・この run で走ったタスク数も言えない（内部では `queues[Q_READY].len()` は 1 命令なのに `pending()` が `bool` に潰す） | rubevy の `FrameStats` に入れたかった 2 つが入らなかった |
| H4 | **`Sym` で ivar を読み書きする口が無い**。`Vm::ivar_get` / `ivar_set` は名前を `&str` で取り、毎回文字列をハッシュする | rubevy の R3: あふれている publish が +10〜14%、落とした 1 件あたり約 12 ns |
| H5 | **2 つの backtrace がネイティブの名前について食い違う**。`Vm::backtrace(Some(mid))` はネイティブ名を先頭に足すが、例外が抱える `Exception#backtrace` には入らない。本家 mruby は C フレームを直下の Ruby フレームに置く | 理由の無い本家との差 |
| H6 | **`tools/check_no_std.sh` が VM の lib しか見ていない**。`sabiruby-serde` も `no_std` なのに対象外 | 規則が機械で確かめられていない |

---

## 2. 決まっていること・既定

### 決まっている（著者の規則）

- unsafe 禁止（VM 本体）。要ると思ったら実装せず止まって報告。
- 根拠のない数を書かない。数は利用者が変えられる場所に置き、理由を残す（出どころが無ければ「不明」）。
- 本家 mruby-task との挙動の差は、正しければ許容（2026-09-17）。ただし理由を `docs/` に書く。
- 版上げと crates.io への公開は著者の指示があってから。

### 既定（違和感があれば止めて報告する）

- **公開 API は足すだけ。** `Vm::ireps` の `#[doc(hidden)] pub` は、H3 の口が入った後も**消さない**（rubevy 0.0.x が読んでいる）。閉じるのは別の機会に著者が決める。
- 本家と同じ意味論を保つ: タスクの起きる順（優先度順、同じ優先度は FIFO のラウンドロビン）、`sleep` の起床の順、キューの `pop` で起きる順。
  **データ構造を変えても起きる順が変わらないこと**が H1 の合否の半分。順が観測できるテストを先に足してから構造を変える。
- H2 は「消してよいとき」を間違えると壊れる（走っているタスク、Proc、Binding、`eval` の使い捨て irep、`require` 済みのファイル）。
  **解放の口はホストが明示的に呼ぶもの**にし、VM が勝手に消さない。安全に言えない場合は止まって案を報告する。

---

## 3. 設計

### 3.1 待ちタスク（H1）

今（`src/builtins/ext_task.rs`）: キューは 4 本の `Vec<ObjId>`（`Q_DORMANT` / `Q_READY` / `Q_WAITING` / `Q_SUSPENDED`）。

- `wake_sleepers`（`:277-300`）は `Q_WAITING` を clone して全部見る。期限が 1 つも来ていなければ先頭のガードで返るが、数百台が眠っていればほぼ毎フレーム誰かの期限が来る。
  → **sleep の期限で引ける構造**（期限の順に並んだもの。`alloc` の `BinaryHeap` か `BTreeMap`）を足し、期限が来たものだけ取り出す。
  同じ期限のタスクが起きる順は今と同じ（`Q_WAITING` に入った順）であること — ヒープは安定でないので、通し番号を鍵に足す。
- `wake_queue_waiters`（`:814-822`）は push のたびに `Q_WAITING` を clone して「このキューを待っているタスク」を探す。
  → **キューごとの待ち手の列**（`Task::Queue` のオブジェクトが自分の待ち手を持つ、または VM 側の `queue ObjId → 待ち手の列` の表）。待ち手が 0 なら何も確保しない。
  GC との関係（待ち手の列は `ObjId` を持つ。ルートかどうか、キューが回収されたときの掃除）を `docs/design/gc.md` の規則に合わせる。
- `q_delete`（`:84-90`）は 4 本を全部歩く。→ **タスクが自分のいるキューを覚える**（既に `status` がある。`status` からキューが決まるなら歩くのは 1 本）。
  1 本の中の位置も歩かずに済ませるかは、測って決める（`Vec` の `remove` は O(N) のまま残る。優先度つきの ready キューの形を変えるのは範囲が広い）。
- `pending()`（`:521-530`）と、`task_run_limits` が締切に気づく粒度: 上の構造が入れば、どちらも歩かずに言えるはず。
- **測るもの**: rubevy の `examples/how_many_scripts`（1000 / 3000 台、`default` / `tight` / `generous` / 問いを出さない対照）と `how_many_subscribers`（読み手が全員 park している行: 100×1、1000×1）を、
  rubevy 側で `[patch.crates-io]` をこの worktree に向けて前後交互に 2 巡。加えて sabiruby 自身の `tools/bench_ab.sh` でタスクを使わないベンチが**遅くなっていない**こと。
  1 台 1 回の費用が台数に比例しなくなることが到達点。

### 3.2 irep を返す（H2）

- まず**誰が irep を指しているか**を全部挙げる: タスクのコンテキストの各フレーム、Proc（`ProcData`）、Binding（`wrap_lvspace` の使い捨て irep）、`eval`、`require`、`Vm::load` の戻り値を持つホスト、デバッグ情報。
  `IrepId` は `Vec` の添字なので、消すなら「番号を再利用しない空き枠」にするか、世代つきの番号にするか。**番号の再利用は、古い `IrepId` を持つホストが別のプログラムを走らせる事故になる**ので、`HostStore` と同じ「閉じた枠」の流儀が第一候補。
- 口の形の案: `Vm::unload(irep: IrepId) -> Result<(), StillInUse>`（そのプログラムの irep の木を、指しているタスク・Proc が 1 つも無いときだけ空き枠にする）。
  「指しているものが無い」を安く言えるか（参照の数を持つか、GC のマークのついでに数えるか）が設計の核。GC は停止型・非移動のマーク＆スイープ（`docs/design/gc.md`）。
- **案を 2 つ以上、代償つきで worklog に書き、決めきれなければ実装せず止まって報告する。** この項目は急がない。間違えると走っているゲームが壊れる。
- rubevy 側（`ScriptWorld` の「内容 → `IrepId`」の表から外す、`replace_script` のあとで古いプログラムを `unload` する）は rubevy の次の計画で。

### 3.3 数える口（H3）

`Vm::irep_count() -> usize`、`Vm::task_counts() -> TaskCounts { ready, waiting, suspended, dormant }`、この run で走ったタスクの数（`task_run_limits` の戻り値を広げるか、`Vm` に「前の run の統計」を持つか。
**既存の戻り値の型を変えない** — 足すだけ）。どれも歩かずに言えること（H1 の構造が入った後なら自然に出る）。rustdoc に「ホストの HUD と計測器のため」。

### 3.4 `Sym` で ivar（H4）

`Vm::ivar_get_sym(obj, Sym)` / `ivar_set_sym(obj, Sym, Value)`（名前は既存の `Vm::intern`・`native_mid` の流儀に合わせる）。`&str` の版は残し、中で `intern` してから `Sym` の版を呼ぶ形に。
測るもの: rubevy の R3 の「あふれるセル」（`how_many_subscribers` の 100×100 以上）が、rubevy 側を `Sym` の版に替えたとき R3 の前の速さに戻ること。rubevy 側の 1 行の差し替えは rubevy の計画で。

### 3.5 backtrace（H5）

`Vm::backtrace(Some(mid))`（`src/vm.rs:2570-2574`）はネイティブ名を先頭に足す。例外が抱える方（`keep_backtrace` → `Exception#backtrace`、`:2589-2600`）は Ruby のフレームだけ。
本家（`mrb_get_backtrace` / `mrb_keep_backtrace`）が C フレームをどう表すかを `ref/` の mruby のソースとフィクスチャで確かめ、同じ形にする。
本家テストとカスタムテスト（`tests/custom/`）に backtrace の文字列を見ているものがあれば基準が動く — 動く場合は理由つきで。
`sabiruby-serde` の `declare` のテストは `Exception#backtrace` の 1 行目を文字列比較している（`serde/tests/declare.rs`）。形が変わったら合わせる。

### 3.6 `check_no_std.sh`（H6）

`sabiruby-serde`（と、`no_std` を名乗るほかの crate があればそれも）を対象に足す。CI（`.github/workflows/ci.yml`）が同じスクリプトを呼んでいるか確かめ、呼んでいなければ合わせる。

---

## 4. 段階

| 段階 | 到達点 | 確認 |
|---|---|---|
| **H6** | `check_no_std.sh` が `sabiruby-serde` も見る | スクリプトが両方で `no_std OK`。わざと `std::` を 1 行足すと落ちることを確かめて戻す |
| **H3** | 数える口（`irep_count`、タスクの数） | テスト。`cargo doc` 警告 0。歩いていないこと（実装を読めば分かる形で） |
| **H4** | `Sym` で ivar | テスト。`&str` の版が同じ結果。micro の前後（`tools/ab_micro.sh` の流儀） |
| **H5** | 2 つの backtrace が同じことを言う | 本家の形の根拠（ソースの場所、フィクスチャの出力）。本家テストの基準が下がらない |
| **H1** | 待ちタスクが O(N) でない | **先に**起きる順を固定するテスト → 構造の変更 → rubevy の計測器で前後交互 2 巡、sabiruby のベンチが遅くなっていない、本家テスト（mruby-task の gem テスト）の基準が下がらない、`Send + Sync`、`no_std` |
| **H2** | irep を返す口 — **まず案の報告で止まる** | 誰が irep を指すかの一覧、案 2 つ以上と代償。実装は著者が案を見てから |

H6・H3・H4・H5 は小さく、互いに独立。H1 がいちばん効き、いちばん広い。H2 はいちばん危ない。**H1 と、ベンチを取るほかの作業を並行させない。**

---

## 5. 分かっている罠

- rubevy の計測器を sabiruby の worktree に向けるには、rubevy の worktree に `.cargo/config.toml` の `[patch.crates-io]` を**コピー**する（cargo の config は cwd から上へ辿る）。コミットしない。
  `sabiruby`・`sabiruby-compiler`・`sabiruby-serde` を同じ所へ向ける（`Vm` の型が 2 つになると rubevy が建たない）。
- 共有の `CARGO_TARGET_DIR` で 2 つの版を建てると 2 つ目が建たず 1 つ目が残る。前の版は別の target で建て、`md5sum` で別物だと確かめる。
- 隣で cargo・docker・ブラウザが動いていると p95 が跳ねる。`pgrep` と `uptime` を計測の前後に見る。
- `wake_sleepers` の先頭のガード（期限が 1 つも来ていなければ即返る）は今も効いている。「眠っているタスクは無料」という rubevy の文書（`docs/host-api.md`「A parked task costs nothing」）は
  命令の予算の話としては正しい。直すのはスケジューラの実時間のほう。
- nil で終わったタスクと Q_DORMANT の件（0.5.1 で直した。`docs/verification/upstream-issue-task-dormant.md`）の周りを触る。回帰テストが `tests/` にある。

## 6. 状況

| 段階 | 状況 |
|---|---|
| H1〜H6 | 未着手（2026-09-20、計画のみ。着手は著者の指示か、rubevy の R10 と games の S5b の後に本体が判断） |

## 7. 気づいた点（段階の報告から本体が集める）

| 日付・段階 | 気づいた点 | どこ | 属する先 | 状況 |
|---|---|---|---|---|
| — | まだ無し | | | |

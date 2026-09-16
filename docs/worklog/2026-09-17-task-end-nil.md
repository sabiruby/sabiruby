# nil で終わったタスクがホストの 1 フレームを食う、終わったタスクが片付かない

2026-09-17。ブランチ `task-end-nil`、main は `9c8ebf2`（0.5.0）。
発端は rubevy_games の庭のデモ（`rubevy_games/docs/worklog/2026-09-17-garden-G4.md` §5a）。
生き物を **10 匹まとめて入れ替えると VM 全体が数秒止まる**、という報告である。
そこに残っていた数字はこうだった。`Vm::task_pending()` は true のまま、`task_run_limits` は
`Ok` を返して 1 命令も走らせず、**入れ替えていない生き物まで凍る**。9 匹なら平気で 10 匹で止まり、
0.4 秒ずつ間隔を空けて入れ替えれば 12 匹でも平気。ゲーム側は「1 フレーム 1 匹・0.4 秒間隔」の
待ち行列で逃げていて、原因は VM 側だろう、と報告だけが上がっていた。

庭の読み方は「古い context の解放が追いつかない」だった。**これは違う。**
以下は、それがなぜ違うか、本当は何だったかの記録である。

## 1. 仮説 — nil を「何も走らなかった」と読んでいないか

`Vm::task_run_limited`（`src/vm.rs:788`）はこう書かれていた。

```rust
if self.task_run_once()?.is_nil() { return Ok(self.instructions - start); }
```

そして `task_run_once`（`src/builtins/ext_task.rs:561`）はこう答える。

```rust
match scheduler_step(vm) {
    Some(t) if td(vm, t).status == DORMANT => Ok(td(vm, t).result.get()),
    _ => Ok(Value::True),
}
```

**終わったタスクの結果**と、**何も ready がなかった**ことが、同じ 1 つの値に乗っている。
`nil` を返すブロックで終わったタスクは、空のスケジューラとまったく同じ nil を答える。
つまり「nil ＝ もう走るものがない」と読むと、**nil で終わるタスク 1 本につきホストの 1 フレームが丸ごと消える**。

rubevy でそれが起きる筋道も読めた。`ScriptTask` を外すと購読が閉じ、`pop` で待っていた反射タスクに
`Rubevy::Unsubscribed` が上がり、**中身のない `rescue` はブロックの値を nil にする**。
生き物 1 匹につき反射 6 本。10 匹で 60 本。60 フレーム。30 fps なら 2 秒である。
9 匹（54 本）で「平気」に見え、10 匹で「止まる」ように見えたのは、
庭が 1 秒に何フレーム回っていたかの差でしかない。間隔を空ければ平気なのも同じ理由で、
1 匹ずつなら 6 フレームの遅れが目に見えないだけである。

## 2. 先に落ちるテストを書く

`tests/task.rs` に 2 本。ホスト側だけで再現するかを確かめたかったので、rubevy は使わない。

nil で終わるタスク 10 本を ready キューの先頭に、その後ろに「回り続けるタスク」を 1 本置き、
命令 200,000 の予算で `task_run_limits` を 1 回呼ぶ。main での結果:

```
the tasks that ended with nil ate the whole frame: 1 of 10 of them finished,
the worker ran 0 instructions, and the frame spent 1 of its 200000
```

**1 命令。** 200,000 の予算を持ったフレームが 1 命令で帰ってきて、後ろのタスクは 0 命令。
庭の `spent 20098 (was 20098)` と同じ顔である（あちらは terminate 済みのタスクが ready の先頭に
いる場合で、`scheduler_step` の `status == DORMANT` の枝が 0 命令で `Some(t)` を返し、
その result の nil がそのままフレームを終わらせていた）。仮説どおり。

2 本目は GC。`stop_task` は終わったタスクを `Q_DORMANT` に移すだけで、そこから出す道は
`Task#close` しかない。そして `gc_mark_roots`（`src/vm.rs:2772`）は 4 本のキューを全部 root にする。
タスクを 1000 本作って終わらせ、`gc_collect` してから生存オブジェクトを数えると
**612 → 2612**。1 本につき 2 個（Task オブジェクトと、その context が持っていた分）、
VM が生きている限り残る。

## 3. 本家はどうしているか（`ref/mruby/mrbgems/mruby-task`）

mruby-task は picoruby 由来だが、`/home/kishima/book/ref/mruby/mrbgems/mruby-task` に
入っているので本家ツリーのほうを読んだ（picoruby 側は見ていない）。

* `mrb_task_mark_all`（`src/task.c:94`）は **4 本のキューを全部歩いて mark する**。
  `q_dormant_` も例外ではない（`include/task.h:170`）。
* さらに `task_create_common`（`src/task.c:938`）が **`mrb_gc_register(mrb, task_obj)`** を呼ぶ。
  タスクオブジェクトは生まれた時点でピン留めされ、`mrb_task_free` の中の
  `mrb_gc_unregister`（`src/task.c:68`）まで外れない。
* その `mrb_task_free` を呼ぶのは `mrb_close_task`（`src/task.c:1692`）だけ。
  `terminate_task_internal` は dormant に移すところで止まる。

**つまり本家も同じように溜める。** しかも二重にピン留めしているので、こちらより強く溜める。
マイコンで決まった本数のタスクを回す使い方では見えない。ゲームが数秒ごとにスクリプトを
入れ替える使い方では、入れ替えるたびに 1 本ずつ残る。
本家に合わせる理由がない側の話なので、`docs/design/gems.md` の
「How far mruby-task may drift」の枠（足すのは自由、壊すのは高くつく）で、
**観測できる振る舞いを変えない範囲で**直すことにした。

## 4. 直し方

### 4a. ホストのループは「走るものがあるか」で終わる

`task_run_once` の答えの読み替えをやめる。`ext_task` に

```rust
pub(crate) enum Step { Ran(ObjId), Idled, Stuck }
```

を置き、`task_step` がスケジューラの 1 回転を **何が起きたか**で答えるようにして、
`task_run_once` はその上の薄い皮にした（外から見える値も意味も今までどおり）。
`task_run_limited` は `Step::Stuck` だけで止まる。
**ホストが時計を持っている場合の意味は変えていない**: ready が空なら、時計を進めるのはホストの仕事なので
`Stuck` であり、今までと同じ場所で帰る。`scheduler_step` が `None`（ready の先頭が呼び出し元自身の
タスク）の場合も `Stuck` にした。ここは今まで `True` を返していて、
`task_run_limited` から入ると無限ループになりうる枝だったので、ついでに塞がっている。

`Vm::task_run_once` の rustdoc には「**nil は『何も走らなかった』ではない**」を書いた。
終わったタスクの結果の nil と、空のスケジューラの nil は、この値だけでは区別できない。
区別が要るループは `while vm.task_pending()` と書く。

同じ読み違いが他にないか `grep -rn task_run_once` で当たった。

* rubevy（`src/lib.rs:493`）— 呼んでいない。「ホストの system から呼ぶな」と書いてある側の言及だけ。
* playground（`sabiruby-playground/wasm/src/lib.rs:418`）— `task_run_budget` を使っているので、
  この修正がそのまま効く。
* **このリポジトリの `tests/host_api.rs`（82・96 行）が同じ書き方だった。**
  `while !vm.task_run_once()?.is_nil() {}`。そこのタスクはたまたま非 nil を返すので通っていたが、
  読む人が真似する場所である。`while vm.task_pending()` に直した。
* `tests/task.rs:23` の同じ形は残した。あれは `task_run_once` の契約そのもの（1 回に 1 本、
  何もなければ nil）を確かめているテストなので、これでいい。

### 4b. dormant キューを弱い参照にする

root から `Q_DORMANT` を外し（`gc_mark_roots` は `queues[1..]` を歩く）、
**mark が終わってから sweep の前に**、誰も mark しなかったタスクをキューから落とす
（`gc_collect`、`self.task.queues[0].retain(|o| heap.is_marked(*o))`）。

捨てた案: 「結果を読んだら落とす」「`Task#join` で落とす」。どちらも
`Task#value` を 2 回読む、`join` しないで `Task.list` で見に行く、といった書き方を壊す。
弱くするなら「**Ruby からも、登録済みのホストのハンドルからも、もう名指せないタスク**」だけが消える。
これは消えても誰にも観測できない。`Task.list`・`Task.get`・`Task.stat`・`Task#status`・`Task#value` は
プログラムが到達できるタスクについて今までどおり答える。

mark の途中ではなく mark 完了後に落とすのは、dormant のタスクを他の誰か（ローカル変数、
`gc_register` したハンドル、`join` 待ちのタスクの `join` フィールド）が指している場合に、
その mark が終わっているようにするため。落とすのは到達不能なものだけなので、
落としたことで新たに mark すべきものが出てくることはない。

## 5. 数字

| | main `9c8ebf2` | このブランチ |
|---|---|---|
| nil で終わるタスク 10 本 + 予算 200,000 命令 1 フレーム | 終わったのは 1 本、後ろのタスク **0 命令**、フレームは **1 命令**で帰る | 10 本すべて終わり、後ろのタスク **199,997 命令**、フレームは 200,007 命令 |
| タスク 1000 本を作って終わらせ `gc_collect` | 612 → **2612** | 612 → **612** |

確認:

* `cargo test --workspace` — 全ファイル通過（新しい 2 本を含む `tests/task.rs` は 24 件）。
* `tools/check_no_std.sh` — `no_std OK`。
* `unsafe` — `src/` に 0 件（`grep -rn unsafe src/` が空）。
* mrbtest の 3 つの基準 — `tools/mrbtest.sh`、`--bytes`、`--no-regexp` を回した。
  **3 つとも、生成される表は 1 行も変わらない**（差分は「Generated by … on」の日付だけ）。
  日付だけの差分を残しても読む人の役に立たないので、`docs/verification/` の 3 ファイルは戻した。
  併せて回帰の下限そのもの（`cargo test --test mrbtest`）を 3 つのビルドで通している
  （既定、`--no-default-features --features std,regexp`、`--no-default-features --features std,utf8`）。
* GC の root を触ったので `SABIRUBY_GC_STRESS=1 cargo test --test mrbtest --release` も回した。通る。
* ベンチは取っていない（この作業では指示されていない）。命令ループには触っていない。

## 6. rubevy 側の再現

`rubevy/tests/restart_burst.rs`（`rubevy/docs/worklog/2026-09-17-restart-burst.md`）。
庭と同じ形——スクリプト 11 本、うち 10 本が「購読 6 本 + `pop` で待つ反射タスク 6 本 + 脳」、
それを 1 フレームで全部入れ替える——で、**触っていない 11 本目が毎フレーム進むこと**を見る。
直す前の VM では **60 フレーム**（10 匹 × 反射 6 本、ちょうど）1 命令も進まない。
直した VM では 0 フレーム。60 という数字が仮説の掛け算とぴったり合ったのが、
この読み方で合っている一番の証拠である。

## 7. 残しておくこと

* **context は増えたままになる。** `create_task` は `vm.contexts` に 1 本積み、
  その index は再利用されない（`gc_collect` が中身を空にするだけ）。
  タスク 1000 本で `contexts` の長さは 1000 増える。中身は空なので大きくはないが、
  1 本ぶんの `Context` は残る。今回の指示の範囲外なので触っていない。庭の
  `contexts 85 live of 99` はこれを見ている。
* **`Task#close` の意味は変えていない。** 明示的に閉じれば今までどおり即座にキューから消え、
  `Task#status` は `ArgumentError` になる。弱い dormant キューはそれとは別の道である。

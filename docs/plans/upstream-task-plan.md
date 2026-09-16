# mruby-task の本家への報告（計画）

作成 2026-09-17。`docs/verification/upstream-pr-candidates.md` の項目 4「終わったタスクが VM の寿命まで残る」について、本家（mruby/mruby、
`mrbgems/mruby-task`、4.1.0-rc `3cf73ee`）への issue と PR の**案を記録として残す**。著者の判断（2026-09-17）: 「本家にもバグはありえる。
正しいと思える動作なら挙動差は許容」「PR は記録だけ」。**送らない**。実行するのは表の 1〜4 のうち手元で閉じるものだけで、
本家への提出（5）は予定に無い。将来送るときの材料が揃っている状態を到達点とする。

同じファイルの項目 1〜3（ホストが 1 ステップずつ回すときの再入、ほか）も同じ手順で扱えるが、まず 4 を通す。

## どれくらい致命的か（先に評価）

* 本家では、終わったタスクは `q_dormant_` に入り、`mrb_task_mark_all` が 4 本のキュー全部を mark し、さらに `task_create_common` の
  `mrb_gc_register` でピン留めされる。外すのは `Task#close` → `mrb_close_task` → `mrb_task_free` だけ。`mrb_task_free` はデータ型の `dfree`
  （`task.c:85`）でもあるので、**オブジェクトが回収されさえすれば context（スタック・callinfo）ごと解放される** — つまりピン留めをやめれば直る構造。
* 漏れるのは Task オブジェクト + `result` + `name` + **context 1 本（スタック込み）**。タスク 1 本あたり数 KB。
* **固定のタスク集合で動く組み込み用途では見えない**。`Task.new` を繰り返す用途（要求ごと・イベントごと・スクリプトの再起動）では**上限なく漏れる**。
  SabiRuby の箱庭（生き物 1 匹の一生ごとに反射タスク 6 本）が踏んだ。PicoRuby 系のアプリで `Task.new` を動的に使うものは同じ。
* 本家の設計が「終了タスクは `close`/`join` で回収する（POSIX のゾンビと同じ）」という意図なら**仕様**であり、その場合は文書化と `join` での自動回収が落とし所。
  意図でなければバグ。**issue ではまずこの点を聞く**（どちらでも patch は用意する）。
* テストで区別できないこと（gem の `test/` に終了タスクの生存を読む assert が無い）は確認済み。

## 手順

| # | 内容 | 状態 |
|---|---|---|
| 1 | 本家で再現・計測（Docker `kishima/mruby:4.1.0-rc` は mruby-task 入り）。`Task.stat[:dormant][:count]` が `GC.start` で減らないこと、`GC.stat` の生存数、RSS（10 万タスクで何 MB か）。`join` しても減らないことも | 未着手 |
| 2 | 既報の確認: `gh issue list -R mruby/mruby --search "task dormant leak"`、PR、picoruby 側（`picoruby/mruby-task` があればそこも）。同じ報告があれば乗る | 未着手 |
| 3 | patch を `ref/mruby` の作業ブランチで書く（2 案。下記）。`rake test` を mruby-task 込みの gembox で（Docker）。gem の test に「終了して参照の無いタスクは GC で消える」を 1 本足す | 未着手 |
| 4 | issue 本文（英語）と PR 説明の下書きを `docs/verification/upstream-issue-task-dormant.md` に。SabiRuby 側の実測（1000 本で 2000 オブジェクト、箱庭の凍結）を証拠として添える | 未着手 |
| 5 | 本家へ送る | **やらない**（著者判断 2026-09-17: 記録だけ） |

## patch の 2 案

**A（最小、port と同じ形）**: `mrb_task_mark_all` の走査から `q_dormant_` を外し、`task_create_common` の `mrb_gc_register` を、タスクが
dormant になる点（`terminate_task_internal` と `execute_task` の末尾）で `mrb_gc_unregister` する。参照の無い終了タスクは次の GC で `dfree` =
`mrb_task_free` が走り、キューから外れて context も解放される。`Task.list`/`Task.get`/`Task.stat` は「Ruby が名指せるもの」だけを見る。
懸念: `mrb_task_free` がキューからの unlink を正しくやるか（`mrb_close_task` 経由でしか呼ばれていない前提のコードが無いか）を読んで確かめる。

**B（ゾンビ設計を保つ）**: 4 本 mark はそのまま、`Task#join` が戻るときと `Task#value` を読んだときに `mrb_close_task` 相当を呼ぶ（回収）。
`join` も `value` も呼ばない書き方（`Task.list` で眺めるだけ）は漏れたままなので、README に「終了タスクは `close` するまで残る」を明記。
本家が「仕様」と答えた場合の落とし所。

**推奨は A**（漏れの原因を消す。観測可能な差は無い）。issue では A を提案し、B を代替として添える。

## 守ること

* `ref/mruby` は参照用クローン。patch は別ブランチで、`ref/mruby` の main は動かさない。
* 本家には送らない（`gh issue create` / `gh pr create` は実行しない）。下書きは記録として `docs/verification/` に置く。
* 過程は `docs/worklog/` に。

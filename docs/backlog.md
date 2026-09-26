# Backlog — 決めて後に回したもの

「やらないと決めたもの」「後で決めるもの」を 1 か所にまとめる。1 項目 1 行で、**何か／なぜ後にしたか／何が起きたら見直すか／詳しい記録**を書く。
やると決めて計画書に移したら、この表から消し、移った先を書く。

作成 2026-09-26（0.7.0 の範囲を決めたとき、`plans/release-0.7-plan.md`）。

## VM の機能

| 項目 | なぜ後に | 見直すとき | 記録 |
|---|---|---|---|
| **段階 3: 暗黙の呼び戻しの中でも待てるようにする**（`join`/`inspect` の `to_s`、`include?` の `==`、Hash の `hash`/`eql?`、`sort` の `<=>`、ホストの関数からの呼び戻し） | ネイティブのフレームを再開できる形（状態機械か `core::future`）にする必要があり、約 130 か所になる。そこで待ちたい例がまだ無い | スクリプトの利用者が、そこで待ちたい場面に当たったとき | `plans/wait-anywhere-plan.md` 段階 3、`design/wait-anywhere.md`「What is still a boundary」 |
| **`sort { }` の比較の中で待つ** | 著者の判断（2026-09-26）: 軽い block で 8.1% 遅くなるので払わない | 段階 3 をやるとき | `verification/bench.md`「After the author's decisions」 |
| `gc_step(work)` とヒープの上限 | 設計の判断が要る（rubevy の outlook で「to do」） | ゲーム側で GC の停止時間かメモリの上限が問題になったとき | rubevy `docs/outlook.md:183-184` |

## 公開 API

| 項目 | なぜ後に | 見直すとき | 記録 |
|---|---|---|---|
| H3: ホストが数を数える公開の口（irep の数、ready のタスク数、この run で走ったタスク数）。今は `#[doc(hidden)] Vm::ireps` | 新しい API で、欲しいのは rubevy の `FrameStats` だけ | rubevy が `FrameStats` に足したくなったとき | `plans/host-scale-plan.md` H3 |
| H4: `Sym` で ivar を読み書きする口 | 新しい API。効くのは rubevy の publish があふれたときだけ（+10〜14%） | rubevy の publish の速さが問題になったとき | `plans/host-scale-plan.md` H4 |
| `Vm::current_line` の名前 | 名前の問題だけ | 次に API を整理するとき | `worklog/2026-09-21-serde-lines.md:418` |

## 同梱物・道具

| 項目 | なぜ後に | 見直すとき | 記録 |
|---|---|---|---|
| `.rbs`（`sig/`）の同梱 | VM の振る舞いに関係しない。型を使う利用者がまだいない | 型検査を使う利用者が出たとき | `plans/from-mrubyedge-plan.md:15` |
| eval の `capture_errors` = FALSE の TODO | 本家と同じ振る舞いで困っていない | eval の誤りの報告を良くしたくなったとき | `plans/eval-require-plan.md:72` |
| wasm の大きさを測っていない | playground の読み込みが問題になっていない | wasm の大きさが問題になったとき | `plans/gems-plan.md:216` |

## 本家（mruby）への報告

| 項目 | なぜ後に | 見直すとき | 記録 |
|---|---|---|---|
| mruby-task の報告候補 4 件（終わったタスクが VM の寿命まで残る、ほか） | 著者の判断（2026-09-17）: 記録だけ | 著者が送ると決めたとき | `verification/upstream-pr-candidates.md`、`plans/upstream-task-plan.md` |
| 境界の内側の `sleep(n)` の後、そのタスクが横取りされなくなる件と、タスクの中の Fiber で異常終了する件 | 記録だけ（2026-09-26） | 同上 | book の `docs/notes/upstream-reports.md` の候補 10・11 |

# Backlog — 決めて後に回したもの

「やらないと決めたもの」「後で決めるもの」を 1 か所にまとめる。1 項目 1 行で、**何か／なぜ後にしたか／何が起きたら見直すか／詳しい記録**を書く。
やると決めて計画書に移したら、この表から消し、移った先を書く。

作成 2026-09-26（0.7.0 の範囲を決めたとき、`plans/release-0.7-plan.md`）。

## VM の機能

| 項目 | なぜ後に | 見直すとき | 記録 |
|---|---|---|---|
| **段階 3: 暗黙の呼び戻しの中でも待てるようにする**（`join`/`inspect` の `to_s`、`include?` の `==`、Hash の `hash`/`eql?`、`sort` の `<=>`、ホストの関数からの呼び戻し） | ネイティブのフレームを再開できる形（状態機械か `core::future`）にする必要があり、約 130 か所になる。そこで待ちたい例がまだ無い | スクリプトの利用者が、そこで待ちたい場面に当たったとき | `plans/wait-anywhere-plan.md` 段階 3、`design/wait-anywhere.md`「What is still a boundary」 |
| **`sort { }` の比較の中で待つ** | 著者の判断（2026-09-26）: 軽い block で 8.1% 遅くなるので払わない | 段階 3 をやるとき | `verification/bench.md`「After the author's decisions」 |
| **Hash の小さい表でもハッシュ値の違うキーに `eql?` をかける**（rc2 の `hash` の assert、`worklog/2026-09-23-rc2.md` の案 (b)） | 0.7.0 の S1 で案 (a) を取った（2026-09-26）。(b) は探索の意味を表の大きさで変える本家の都合を真似ることになり、CRuby とも違う。探索の速さ（stage 2c）にも触れる | 本家が AR でもハッシュ値を比べるようになったとき、または利用者がこの違いに当たったとき | `worklog/2026-09-26-release-0.7-a.md`、`verification/mrbtest-notes.md` の `hash` |
| **「hash modified」を世代の数で見る**（今は要素数だけ。`eql?` の中で 1 つ消して 1 つ足すと気づかない） | Hash を書き換える道（`[]=`、`delete`、`rehash`、`replace`、`clear`、`shift`、`compact!`、`select!` ほか）すべてで数を進める必要があり、1 つ漏れると検査が黙って効かなくなる。今でも範囲外は読まない（Rust の `Vec`）ので、困るのは本家と答えが違うことだけ | 本家のテストか利用者が、数の変わらない書き換えで違いに当たったとき | `worklog/2026-09-26-release-0.7-a.md` 気づいた点 5 |
| **`eval` と `binding` の irep を返さない**（`ext_eval.rs`、`ext_binding.rs` の `wrap_lvspace`。呼ぶたびに `ireps` が伸びる） | 番号を誰にも渡さないので、いつ使われなくなったかは回収（GC）でしか分からない。GC に irep の印と掃除を足すことになり、`Vm::unload` の「GC の規則を変えない」を崩す | スクリプトが `eval` や `binding` をループの中で繰り返し呼ぶ例が出たとき | `worklog/2026-09-26-release-0.7-b.md` 気づいた点、`design/gc.md`「Programs」 |
| **backtrace の残りの違い 7 つと、`instance_exec`・top level の `eval` の中の `__method__`**（本家は `:instance_exec`／`:eval`、ここは `nil`） | どちらも本家が C 関数のフレームを書き換えて名前を運ぶ形で、揃えるには `CallInfo` に 1 つ足すか `Cci` を増やす — フレームの形を変える | フレームの形を変える仕事（段階 3 の再開など）が来たとき、または利用者がこの違いに当たったとき | `worklog/2026-09-26-release-0.7-b.md` S5「残った違い」・気づいた点、`tests/custom/native_backtrace.rb`、`design/gems.md` |
| **記録のイベント（`TraceEvent::Raise`／`CatchLook`）が `unload` した irep を指しうる** | 表示にしか使わず、行が `None` になるだけで壊れない | デバッガがこの行を必要としたとき | `worklog/2026-09-26-release-0.7-b.md` 気づいた点（`src/inspect.rs`） |
| **`load_and_run` で読んだプログラムは `unload` できない** | 番号を返さない口なので当然。起動時に 1 度読む層やデータなら困らない。何度も読み直すのは rubevy（`ScriptWorld::load_and_run`）と Factory の側で `load` + `run_irep` + `unload` に替える話 | rubevy・games が同じものを何度も読み直すとき | `worklog/2026-09-26-release-0.7-b.md` 気づいた点 |
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
| README の数（fixtures の本数、mrbtest の通過数、gem の数など）を CI で数え直す | 0.7.0 の S7 で見送った（2026-09-26）。いちばん古くなりやすい mrbtest の数は本家の `mrbc`（Docker）で作った表から出るので CI では数え直せず、数え直せる数だけ見ると「確かめた」ように見えて一番ずれやすいものが漏れる | README の数のずれがまた見つかったとき、または mrbtest の表を CI で作れるようになったとき | `worklog/2026-09-22-readmes.md` 気づいた点 3、`worklog/2026-09-26-release-0.7-a.md` |
| wasm の大きさを測っていない | playground の読み込みが問題になっていない | wasm の大きさが問題になったとき | `plans/gems-plan.md:216` |

## 本家（mruby）への報告

| 項目 | なぜ後に | 見直すとき | 記録 |
|---|---|---|---|
| mruby-task の報告候補 4 件（終わったタスクが VM の寿命まで残る、ほか） | 著者の判断（2026-09-17）: 記録だけ | 著者が送ると決めたとき | `verification/upstream-pr-candidates.md`、`plans/upstream-task-plan.md` |
| 境界の内側の `sleep(n)` の後、そのタスクが横取りされなくなる件と、タスクの中の Fiber で異常終了する件 | 記録だけ（2026-09-26） | 同上 | book の `docs/notes/upstream-reports.md` の候補 10・11 |

## 性能（原因が分からないまま受け入れたもの）

| 項目 | なぜ後に | 見直すとき | 記録 |
|---|---|---|---|
| 0.7.0 で block なしの整数の `sort` のマイクロベンチが +9%、rubevy の `publish_heard`（購読 10 以上・1 フレーム 100 通以上）が +11〜15% | 著者の判断（2026-09-27）: 現状で受け入れる。S4・S5・S6 のどれでもなく、命令数は同じ、VM 単体の同じ操作は遅くない。深追いしない | `perf` が使えるようになったとき、または実際のゲームで publish の遅さが問題になったとき | `verification/bench.md` の 0.7.0 の節、`worklog/2026-09-27-release-0.7-bench.md` |

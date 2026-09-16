# 本家 mruby-task の「終わったタスクが残る」を、記録として作りきる

2026-09-17。ブランチ `upstream-record`、sabiruby main は `bd6829b`。
`docs/plans/upstream-task-plan.md` の手順 1〜4。5（本家へ送る）は**やらない**（著者判断:
「本家にもバグはありえる。PR は記録だけ」）。`gh issue create` / `gh pr create` は一度も実行していない。
読むほう（`gh issue list` / `gh pr list` / `gh issue view` / `gh pr view` / `gh repo list`）だけ使った。

Rust には触っていない。sabiruby の `src/` は 1 行も変えていないので `cargo test` は回していない
（回す理由が無い）。変えたのは `docs/` と、参照用クローンの作業ブランチだけである。

## 0. 作業場所 — `ref/mruby` を汚さずに patch を書く

`/home/kishima/book/ref/mruby` は 4.1.0-rc（`3cf73ee`）の detached HEAD で、**完全なクローン**
（`git rev-parse --is-shallow-repository` は false、`origin/master` ほかのブランチもある）。
指示どおり、そこは動かさず `git worktree add <scratch>/mruby-task-patch -b task-dormant-weak` で
別の作業木を作った。作業木は最後に `git worktree remove` したが、**ブランチ `task-dormant-weak` は
`ref/mruby` に残してある**（patch のコミット `3749292` がそこにある）。

作業木には落とし穴が 2 つあった。どちらも「submodule は worktree に付いてこない」に由来する。

1. `mrbgems/mruby-compiler/lib/prism` が空で、`rake` が `git submodule update --init` を走らせ、
   作業木の `.git` がコンテナから見えないので落ちる。`ref/mruby` 側の prism をコピーして解決した
   （`templates/template.rb` があれば `prism_submodule` タスクは何もしない）。
2. コピーした prism の中の `.git` ファイルが `../../../../.git/modules/...` を指していて、
   作業木で `git status` すら通らなくなった。その `.git` を消して普通のディレクトリにした。

## 1. 本家で再現する

Docker の `kishima/mruby:4.1.0-rc` に入っている `mruby` は **mruby-task を含まない**
（`mruby -e "p Task"` が `uninitialized constant Task`）。mruby-task はどの gembox にも入っていない
（`grep task mrbgems/*.gembox` が空、`mrbgem.rake` を見ても gembox 登録は無い）ので、当然ではある。
なのでコンテナの中で build_config を書いて本家をビルドした。中身はこれだけ:

```ruby
MRuby::Build.new do |conf|
  conf.toolchain
  conf.gembox 'default'
  conf.gem core: 'mruby-task'
  conf.enable_test
end
```

ここでもう 1 つ引っかかった。最初はスクラッチパッドを `/w` にマウントしていて、リンクが
`-lws2_32 -lwinmm` を要求して落ちた。`lib/mruby/gem.rb:148` の `for_windows?` が
`('a'..'z').any? { |v| Dir.exist?("/#{v}/") }` で Windows を判定している——MSYS の `/c/` を見るための
判定だが、**`/w` というマウントがあるだけで本家のビルドは自分を Windows だと思う**。
マウント先を `/src` にしたら通った。本家の小さな瑕疵で、今回の主題ではないので報告はしない（記録だけ）。

計測用の Ruby は `File.read("/proc/self/status")` で VmRSS / VmHWM を読む
（`/usr/bin/time` もコンテナの `sh` の `time` も無い）。`GC.stat[:live]`、
`ObjectSpace.count_objects` はどちらも使える。ループの前後を比べ、最後に必ず `GC.start` してから測った。

| mode（200 周 × 10 本 = 2000 本） | dormant | live の増分 | RSS の増分 |
|---|---|---|---|
| 何もしない | 2000 | +6444 | +2120 kB |
| 別タスクから `join` する | 2200 | +7444 | +2388 kB |
| 後で `value` を読む | 2000 | +6444 | +2120 kB |
| 後で `close` する | 0 | +43 | 0 kB |
| 10000 本、何もしない | 10000 | +32044 | +10608 kB |
| 100000 本、`close` する | 0 | +43 | 0 kB |

**`join` は回収しない**（しかも join する側のタスクが 200 本まるごと残る）。`value` も回収しない。
`close` だけが効く。1 本あたり RSS 約 1 kB、生存オブジェクト 3.2 個。

**10 万本は `close` 無しでは走りきらない。** 予想していなかった落ち方をした:

```
died after 65500 tasks spawned from the same block: RuntimeError: too many irep references
```

終わったタスクが proc を掴んだままなので、**同じブロックから 65535 回 task を作ると irep の
参照カウント（`src/state.c:108`）が溢れる**。メモリが足りる環境でも、長命なプログラムは
そこで死ぬ。漏れが「遅くなる」ではなく「落ちる」に変わる境界が具体的な数字で出たのは収穫で、
issue の本文にもそのまま入れた。

`README.md` の契約も確かめた（これが「バグか仕様か」を分ける）。**close は要求されていない。**
それどころか `Task#close` は README に**存在しない**。「Task Instance Methods」の一覧は
`#status` `#name` `#priority` `#suspend` `#resume` `#terminate` `#join` だけで、`#value` も無い。
`close` が出てくるのは `Task::Queue#close` だけ。唯一近いのは "GC Integration" の
"Tasks and their stacks/callinfo are properly marked and freed" で、**実際の振る舞いと逆のことを
読者に言っている**。

## 2. 既報を当たる

`gh issue list -R mruby/mruby --search "task" --state all`（50 件）、`"dormant"`、`"task leak"`、
`gh pr list -R mruby/mruby --search "mruby-task" / "dormant" / "task close"`、`gh repo list picoruby`。
**この漏れの報告は無い。** 出てきたもので効いたのは 2 本。

**PR #6947「Feature: Task#close」**（hasumikin、2026-07-14 merge）。`Task#close` を足した当人が、
理由をこう書いている:「R2P2（PicoRuby）ではシェルのコマンド 1 つがタスク 1 本を生む。この機能が
無いと、コマンドを実行するたびに Task オブジェクトが漏れていた。`Task#terminate` は Dormant に移すが、
**参照を保持しておきたい用途があるので**自動では解放できない。だから明示的に閉じる API が要る」。
つまり**本家はこの漏れを知っていて、明示 API を選んだ**。しかもその理由（参照が要るかもしれない）は、
到達可能性がまさに答えるもので、全部ピン留めするより正確に答える。issue はこれを引用して、
「設計なのか」をこの一文に対して問う形にした。

**PR #6983「Detach envs from a task stack before freeing it」**（同、2026-07-31）。
patch A の安全性の核心がここにあった:「`mrb_task_free` からは呼ばない。タスクは生涯 GC に register
されているので、GC がタスクを解放するのはヒープを畳むときだけであり、そこで detach するのは
無意味（後で mark されない）でも危険（env のページが既に無いかもしれない）でもある」。
**`mrb_task_free` は「close 経由でしか呼ばれない」ことを本当に前提にしている**。
dormant を弱くすると、この前提が成り立たなくなる。計画書が懸念として書いていたとおりだった。

ほかに #6931（mark 中に tick IRQ を止める）、#6930（context 初期化の OOM）、#6922（sleep が別の
タスクを止める／terminate の自己判定が死んでいる）、#6863/#6865/#6870/#6872/#6886/#6642
（mruby-task の segv や `MRB_TT_FREE` assertion）。どれも dormant キューの寿命には触れていない。
picoruby org に `mruby-task` repo は無い（34 repo を見た）。gem は本家にあり、`picoruby/picoruby` が
それを使う側である。

## 3. patch — 計画書の A は、そのままでは壊れる

まず計画書どおりの最小の A を書いた。(i) `mrb_task_mark_all` の走査から `q_dormant_` を外す、
(ii) dormant になる 4 か所で `mrb_gc_unregister`、(iii) `dfree` がキューから unlink して env を detach する
（#6983 のとおり、ヒープ解体中は detach しない。`mrb_mruby_task_gem_final` でフラグを立てる。
gem の final は atexit として `mrb_close` の `mrb_protect_atexit` で走り、`free_heap` より前なので間に合う）。

漏れは消えた（2000 本で dormant 0、RSS 増分 0）。**が、壊れていた。**
参照が残っているタスクを 1 本だけ確かめる小さなスクリプトで:

```
t = Task.new { "result-#{1 + 1}" }; Task.run; GC.start; p t.value
=> "646"
```

`"result-2"` ではなく `"646"`。**解放済みのスロットを読んでいる。** 原因は 1 行で言える:
**mruby のデータ型には mark フックが無い**（`mrb_data_type` は `struct_name` と `dfree` だけ）。
だから `mrb_task` の `result` と `name`、それにスタックは、**`mrb_task_mark_all` のキュー走査からしか
到達できない**。dormant を走査から外した瞬間、まだ生きている Task の中身までマークされなくなる。
Rust の移植側で同じ形が通ったのは、あちらの Task オブジェクトが result も name もオブジェクトグラフで
持っているからで、C ではそこが違う。**計画書の A はこの 1 点で不足している。**

`final_marking_phase` でも `mrb_task_mark_all` が呼ばれる（`src/gc.c:1521`）ので、
「そこで self が既に mark 済みの dormant タスクだけ中身を mark する」も考えたが、
この時点で gray スタックはまだ流し切られていない（`gc_mark_gray_list` はこの関数の末尾）。
gray に積まれただけの参照元から辿られる Task は白のままなので、**1 パスでは決まらない**。捨てた。

採ったのは「中身をオブジェクトグラフに載せる」ほう。dormant に移る**直前**に（まだマークされる
キューにいるうちに、ここが肝で、`mrb_iv_set` も `mrb_env_detach_all` も割り当てるので GC が走りうる）、

1. `result` と `name` を wrapper オブジェクトの ivar (`MRB_IVSYM(result)` / `MRB_IVSYM(name)`) に移す。
   構造体のフィールドは C が読むためにそのまま残す。ivar は「オブジェクトグラフに載せる」ためだけのもの。
2. スタックから逃げた env を detach する（#6983 と同じ理由。**VM が今その context に立っている
   タスク——自分を terminate したタスク——は除く**。`execute_task` の末尾が `mrb->c` を戻したあとに
   もう一度同じ関数を呼ぶので、そこで detach される）。
3. `mrb_gc_register` のピンを落とす。

の 3 つを行う `task_retire()` を置いた。`Task#name=` は dormant なタスクにも効くので、
そこでも ivar を書くようにした（フィールドだけ書くと新しい name が誰にもマークされない）。

これで `t.value` は `"result-2"` を返し、漏れも消えた。逃げたクロージャ（`$esc = -> { a }`）も、
タスクを捨てたあと GC を 2 回回しても `"escaped-4"` を返す。

`Task#close` の意味は変えていない。`Task.list` / `Task.get` / `Task.stat` / `Task#status` /
`Task#value` は、プログラムが到達できるタスクについて今までどおり答える。
変わるのは (a) 誰も到達できない終了タスクが消えること、(b) C の埋め込み側が終了タスクのハンドルを
GC をまたいで持つなら `mrb_gc_register` すること、の 2 つだけである。

## 4. テストと数字

gem のテスト（`test/gc_task.rb`）に 2 本足した。1 本は指示された
「終了して参照の無いタスクは GC で消える」。もう 1 本は上の (2) の穴を塞ぐ番人——
「参照の残っている終了タスクは GC のあとも result を答える」。

* **patch 前の本家で新しいテストが落ちること**を確かめた（C の変更だけ `git stash` して再ビルド）:
  `Fail: a finished task nothing refers to is collected` / KO 1。番人のほうは本家でも通る（当然）。
* patch あり: `rake test`（default gembox + mruby-task）で **Total 2700 / OK 2632 / KO 0 / Crash 0**。
  patch 前の同じビルドが Total 2698 / OK 2630 / KO 0 なので、増えた 2 件は新しいテストそのもの。
* GC の root を触る変更なので **`MRB_GC_STRESS` のビルド**（`build_config` 別、`-g` 付き）でも回した:
  **Total 929 / OK 920 / KO 0 / Crash 0**（gem が少ない構成なので総数が違う）。
  再現スクリプトも stress ビルドで走らせ、`value` も逃げたクロージャも正しく、
  500 本流したあとの dormant は 1（まだ参照の残っている 1 本だけ）。
* 再現の全モード（plain / join / value / close）と 10000 本・100000 本を patch 後に回すと、
  **すべて dormant 0、live +43、RSS 増分 0 kB**。`too many irep references` で死んでいたループは
  **20 万本走りきった**。

ベンチは取っていない（指示に無い。命令ループにも触っていない）。

## 5. 記録の置き場所

* [`../verification/upstream-issue-task-dormant.md`](../verification/upstream-issue-task-dormant.md)
  — issue 本文（英語）、PR 説明の下書き、既報の調査、README の契約。冒頭に
  **「記録のみ・未提出（著者判断 2026-09-17）」**を明記した。
* [`../verification/patches/mruby-task-dormant-weak.patch`](../verification/patches/mruby-task-dormant-weak.patch)
  — `git format-patch` 形式。`ref/mruby` のブランチ `task-dormant-weak`（コミット `3749292`）と同じもの。
  コミットメッセージ末尾の attribution 2 行は、ここの規則に従って付けてある。**実際に出すときは外す。**
* patch B（`join` / `value` で回収する案）は**書いていない**。A が安全な形で通ったからで、
  かつ B は「join も value も呼ばないコード」（今回の再現がまさにそれ）の漏れを直さない。
  issue には代替として文章で添えた。

## 6. 残したこと・気づいたこと

* **`for_windows?` がマウント名で誤判定する**（§1）。本家の瑕疵だが今回の主題ではない。
  `upstream-pr-candidates.md` に項目として足すかは著者の判断なので、ここに書くだけにした。
* **`mrb_task_init_context`（picoruby-sandbox のタスク再利用）との関係は読んだだけで、試していない。**
  retire したタスクを再初期化する道は、context を作り直すので patch と衝突しないはずだが、
  Ruby から叩けないので gem のテストには現れない（`test/proc_set_stack.rb` が C ヘルパ経由で触っている）。
  その 87 行のテストは今回の 2700 件に含まれていて、通っている。
* **`Task.list` に現れる終了タスクの集合が変わる**。プログラムが名指せるものだけになる。
  README は `Task.list` を "all tasks (including dormant tasks)" と書いているので、
  本家に出すなら README の 1 行も直すべきである（patch には入れていない。issue の問いの答え次第で
  文面が変わるため）。

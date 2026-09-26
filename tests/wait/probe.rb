# Waits from inside a block (or a method) reached by one path, in a task, and records whether
# the task really went back to the scheduler. The shape and the judgment are those of the
# measurements on the reference (mruby 4.1.0-rc2): the task `main` waits, the task `other`
# marks every 20 ms and pushes (or resumes `main`) at its third mark. `main` waited when marks
# of `other` lie between its start and its end.
#   sabiruby tests/wait/probe.rb <path> <pop|sleep|sleep0|pass|usleep|sleep_ms>
# (`host` is the sixth way, for a host that answers on a queue of its own: `tests/wait_anywhere.rs`)
#   sabiruby tests/wait/probe.rb list
# `tests/wait_anywhere.rs` runs every pair and holds what each one must do.

def via_yield
  yield
end

def via_block_call(&b)
  b.call
end

def body_in_method
  $body.call_body
end

class Ghost
  def method_missing(name, *args, &b)
    b.call
  end
end

def ensure_return
  begin
    return
  ensure
    $body.call_body
  end
end

class Host
  define_method(:dm) { $body.call_body }
end

class Waiter
  def initialize(x)
    $body.call_body
    @x = x
  end
  attr_reader :x
end

class YieldingInit
  def initialize
    yield
  end
end

class Tos
  def to_s
    $body.call_body
    "t"
  end
end

class Cmp
  def ==(o)
    $body.call_body
    true
  end
end

PATHS = {
  "direct"          => -> { $body.call_body },
  "yield"           => -> { via_yield { $body.call_body } },
  "proc_call"       => -> { pr = proc { $body.call_body }; pr.call },
  "block_call"      => -> { via_block_call { $body.call_body } },
  "array_each"      => -> { [1].each { $body.call_body } },
  "hash_each"       => -> { {a: 1}.each { $body.call_body } },
  "times"           => -> { 1.times { $body.call_body } },
  "map"             => -> { [1].map { $body.call_body } },
  "loop"            => -> { loop { $body.call_body; break } },
  "tap"             => -> { 1.tap { $body.call_body } },
  "c_array_index"   => -> { [1].index { $body.call_body; true } },
  "c_array_new"     => -> { Array.new(1) { $body.call_body } },
  "c_sort"          => -> { [2, 1].sort { |a, b| $body.call_body; a <=> b } },
  "send"            => -> { send(:via_yield) { $body.call_body } },
  "__send__"        => -> { __send__(:via_yield) { $body.call_body } },
  "public_send"     => -> { public_send(:tap) { $body.call_body } },
  "send_missing"    => -> { Ghost.new.send(:nosuch) { $body.call_body } },
  "method_missing"  => -> { Ghost.new.nosuch { $body.call_body } },
  "instance_exec"   => -> { Object.new.instance_exec { $body.call_body } },
  "instance_eval"   => -> { Object.new.instance_eval { $body.call_body } },
  "class_exec"      => -> { Class.new.class_exec { $body.call_body } },
  "module_eval"     => -> { Module.new.module_eval { $body.call_body } },
  "class_eval"      => -> { Class.new.class_eval { $body.call_body } },
  "method_call"     => -> { method(:body_in_method).call },
  "method_call_blk" => -> { method(:via_yield).call { $body.call_body } },
  "bind_call"       => -> { Object.instance_method(:body_in_method).bind_call(self) },
  "define_method"   => -> { Host.new.dm },
  "class_new"       => -> { Class.new { $body.call_body } },
  "module_new"      => -> { Module.new { $body.call_body } },
  "rescue"          => -> { begin; raise "x"; rescue; $body.call_body; end },
  "ensure_raise"    => -> { begin; raise "x"; ensure; $body.call_body; end },
  "ensure_break"    => -> { [1].each { begin; break; ensure; $body.call_body; end } },
  "ensure_return"   => -> { ensure_return },
  "c_each_object"   => -> { done = false
                            ObjectSpace.each_object(Class) { next if done; done = true; $body.call_body } },
  # the reference's `new` is bytecode, so `initialize` is an ordinary frame there
  "new_initialize"  => -> { Waiter.new(1) },
  "new_yield"       => -> { YieldingInit.new { $body.call_body } },
  "eval_string"     => -> { eval("$body.call_body") },
  "instance_eval_s" => -> { Object.new.instance_eval("$body.call_body") },
  # more block-taking builtins
  "c_find_index"    => -> { [1].find_index { $body.call_body; true } },
  "c_rindex"        => -> { [1].rindex { $body.call_body; true } },
  "c_sort_by"       => -> { [2, 1].sort_by { |a| $body.call_body; a } },
  "c_select!"       => -> { [1].select! { $body.call_body; true } },
  "c_reject!"       => -> { [1].reject! { $body.call_body; false } },
  "c_keep_if"       => -> { [1].keep_if { $body.call_body; true } },
  "c_delete_if"     => -> { [1].delete_if { $body.call_body; false } },
  "c_count"         => -> { [1].count { $body.call_body; true } },
  "c_to_h"          => -> { [1].to_h { |x| $body.call_body; [x, x] } },
  "c_ary_fetch"     => -> { [].fetch(0) { $body.call_body } },
  "c_ary_delete"    => -> { [].delete(0) { $body.call_body } },
  "c_hash_default"  => -> { Hash.new { |h, k| $body.call_body }[:k] },
  "c_hash_fetch"    => -> { {}.fetch(:k) { $body.call_body } },
  "c_hash_delete"   => -> { {}.delete(:k) { $body.call_body } },
  "c_hash_merge"    => -> { {a: 1}.merge({a: 2}) { $body.call_body; 3 } },
  "c_hash_update"   => -> { {a: 1}.update({a: 2}) { $body.call_body; 3 } },
  "c_struct_new"    => -> { Struct.new(:a) { $body.call_body } },
  "c_catch"         => -> { catch(:t) { $body.call_body } },
  "c_hash_default_m"=> -> { Hash.new { |h, k| $body.call_body }.default(:k) },
  "c_data_define"   => -> { Data.define(:a) { $body.call_body } },
  "c_each_char"     => -> { "a".each_char { $body.call_body } },
  "c_each_byte"     => -> { "a".each_byte { $body.call_body } },
  "c_str_upto"      => -> { "a".upto("a") { $body.call_body } },
  "c_regexp_match"  => -> { /a/.match("a") { $body.call_body } },
  "c_sub"           => -> { "a".sub(/a/) { $body.call_body; "b" } },
  "c_gsub"          => -> { "a".gsub(/a/) { $body.call_body; "b" } },
  "c_scan"          => -> { "a".scan(/a/) { $body.call_body } },
  # callbacks a native makes on its own (stage 3 of the plan: these stay boundaries)
  "cb_to_s"         => -> { [Tos.new].join },
  "cb_eq"           => -> { [Cmp.new].include?(1) },
}

class Body
  def initialize(kind, q, log)
    @kind, @q, @log = kind, q, log
  end

  def call_body
    @log << [:start, Task.tick]
    case @kind
    when "pop"      then @q.pop
    when "sleep"    then sleep 0.1
    when "sleep0"   then sleep
    when "pass"     then Task.pass
    when "usleep"   then usleep 100_000
    when "sleep_ms" then sleep_ms 100
    when "host"     then @answer = ask(:probe).pop
    end
    @log << [:end, Task.tick, @answer]
  end
end

path, kind = ARGV
if path.nil? || path == "list"
  puts PATHS.keys.join(" ")
else
  q = Task::Queue.new
  log = []
  $body = Body.new(kind, q, log)

  main = Task.new(name: "main") do
    begin
      PATHS.fetch(path).call
      log << [:returned]
    rescue => e
      log << [:raised, e.class, e.message]
    end
  end

  report = -> do
    s = log.index { |e| e[0] == :start }
    e = log.index { |e| e[0] == :end }
    raised = log.find { |e| e[0] == :raised }
    verdict = if s && e && kind == "host"
                # the host answers only after the task has parked on the empty queue
                log[e][2] == :answer ? "waited" : "answered #{log[e][2].inspect}"
              elsif s && e
                # waited: marks of `other` between main's start and its end
                log[s..e].any? { |x| x[0] == :other } ? "waited" : "did not wait"
              elsif raised then "raised #{raised[1]}: #{raised[2]}"
              else "no end"
              end
    # the ensure path re-raises what it was unwinding once the wait is over
    verdict += ", then raised #{raised[2]}" if s && e && raised
    puts "#{path} #{kind}: #{verdict}"
  end

  if kind == "host"
    # `ask` is the host's (a queue it answers on, as rubevy's `Rubevy.ask`), and so is the
    # scheduler: it runs the tasks, answers, and calls this once `main` is done
    $report = report
  else
    other = Task.new(name: "other") do
      5.times do |i|
        log << [:other, Task.tick]
        if i == 2
          q.push(:pushed)
          main.resume if main.status == :SUSPENDED
        end
        sleep 0.02
      end
    end
    Task.run
    report.call
  end
end

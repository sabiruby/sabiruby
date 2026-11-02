# checkpoint-test: suspend-after "c wakes 0"
# checkpoint-test: clock virtual
# Tasks that sleep. Who wakes first is decided by the clock and, on the same
# tick, by priority (the virtual clock of the test build makes the ticks the
# same in every run). The program suspends inside a Task while the others
# sleep, and again while one is ready to run: the waits go on with the ticks
# they had left.
def run_tasks(*tasks)
  tasks.each(&:join)   # the program is a Task (checkpoint run --as-task)
rescue RuntimeError
  Task.run             # the program is at the root
end

log = []
a = Task.new(name: "a", priority: 140) do
  3.times do |i|
    sleep_ms 40
    log << "a wakes #{i}"
    puts log.last
  end
  :a_done
end
b = Task.new(name: "b", priority: 130) do
  2.times do |i|
    sleep_ms 60
    log << "b wakes #{i}"
    puts log.last
  end
  :b_done
end
c = Task.new(name: "c", priority: 150) do
  2.times do |i|
    sleep_ms 50
    log << "c wakes #{i}"
    puts log.last
    Checkpoint.suspend("task_sleep.ckpt")   # a and b are sleeping
  end
  :c_done
end
d = Task.new(name: "d", priority: 160) do
  sleep_ms 120
  # b woke on this tick too (b has the higher priority): it ran first
  puts "d wakes with #{log.size} wakes before"
  Checkpoint.suspend("task_sleep.ckpt")
  :d_done
end
run_tasks(a, b, c, d)
p [a.value, b.value, c.value, d.value]
p [a.status, b.status, c.status, d.status]

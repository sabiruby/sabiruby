# checkpoint-test: suspend-after "suspended: SUSPENDED SUSPENDED"
# checkpoint-test: clock virtual
# Task#suspend and Task#resume. The boss suspends a worker that takes turns
# with it, and a sleeper in the middle of its sleep (which keeps its wait).
# The program suspends while both are suspended, and again after the sleeper
# is resumed with its deadline passed.
def run_tasks(*tasks)
  tasks.each(&:join)   # the program is a Task (checkpoint run --as-task)
rescue RuntimeError
  Task.run             # the program is at the root
end

worker = Task.new(name: "worker", priority: 150) do
  5.times do |i|
    puts "worker #{i}"
    Task.pass
  end
  :worker_done
end
sleeper = Task.new(name: "sleeper", priority: 140) do
  sleep_ms 20
  puts "sleeper woke"
  :sleeper_done
end
boss = Task.new(name: "boss", priority: 150) do
  Task.pass
  Task.pass
  worker.suspend
  sleeper.suspend
  puts "suspended: #{worker.status} #{sleeper.status}"
  Checkpoint.suspend("task_suspend.ckpt")   # worker suspended, sleeper suspended while sleeping
  sleep_ms 40
  puts "boss resumes the worker"
  worker.resume
  worker.join
  puts "boss resumes the sleeper"
  sleeper.resume
  Checkpoint.suspend("task_suspend.ckpt")   # the sleeper waits, its deadline passed
  sleeper.join
  :boss_done
end
run_tasks(boss, worker, sleeper)
p [boss.value, worker.value, sleeper.value]

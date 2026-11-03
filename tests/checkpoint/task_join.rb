# checkpoint-test: suspend-after "worker 1 step 1"
# A Task waits for another to end (Task#join). The program suspends while
# the joining Task waits, and again after the one it waited for has ended
# and woken it.
def run_tasks(*tasks)
  tasks.each(&:join)   # the program is a Task (checkpoint run --as-task)
rescue RuntimeError
  Task.run             # the program is at the root
end

workers = (1..2).map do |n|
  Task.new(name: "worker #{n}", priority: 141) do
    3.times do |i|
      puts "worker #{n} step #{i}"
      Checkpoint.suspend("task_join.ckpt") if n == 1 && i == 1   # the waiter waits on a join
      Task.pass
    end
    n * 10
  end
end
waiter = Task.new(name: "waiter", priority: 130) do
  workers.each do |w|
    w.join
    puts "#{w.name} ended with #{w.value}"
  end
  Checkpoint.suspend("task_join.ckpt")
  workers.sum(&:value)
end
run_tasks(waiter, *workers)
p waiter.value
p workers.map(&:status)

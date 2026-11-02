# checkpoint-test: suspend-after "a 1"
# checkpoint-test: clock virtual
# The image is taken where the scheduler has asked for a switch: the Task's
# time ends right at that safe point, on the writing side and on the
# restoring one (Checkpoint.__pass_and_suspend, of the test builds: Task.pass
# and Checkpoint.suspend in one call). Two Tasks of one priority take turns;
# the other one must run next after each restore, as without one. The second
# image is taken after a Task of higher priority was made (the scheduler asked
# for a switch there too): it runs first.
def run_tasks(*tasks)
  tasks.each(&:join)   # the program is a Task (checkpoint run --as-task)
rescue RuntimeError
  Task.run             # the program is at the root
end

a = Task.new(name: "a", priority: 150) do
  3.times do |i|
    puts "a #{i}"
    Checkpoint.__pass_and_suspend("task_switch.ckpt") if i == 1
    Task.pass
  end
  :a
end
b = Task.new(name: "b", priority: 150) do
  3.times do |i|
    puts "b #{i}"
    if i == 1
      Task.new(name: "c", priority: 140) { puts "c runs first" }
      Checkpoint.suspend("task_switch.ckpt")   # c is ready and goes before b
    end
    Task.pass
  end
  :b
end
run_tasks(a, b)
p [a.value, b.value]

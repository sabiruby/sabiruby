# checkpoint-test: suspend-after "task a: fiber 2"
# Fibers inside Tasks: Task a steps a Fiber, Task b sleeps. The program
# suspends inside the Fiber (the Task's own context waits in Fiber#resume),
# and in the Task between two resumes.
def run_tasks(*tasks)
  tasks.each(&:join)   # the program is a Task (checkpoint run --as-task)
rescue RuntimeError
  Task.run             # the program is at the root
end

a = Task.new(name: "a", priority: 140) do
  f = Fiber.new do
    1.step do |i|
      puts "task a: fiber #{i}"
      Checkpoint.suspend("task_fiber.ckpt") if i == 2   # in the Fiber of a Task
      Fiber.yield i
    end
  end
  sum = 0
  4.times { sum += f.resume }
  Checkpoint.suspend("task_fiber.ckpt")   # the Fiber is suspended
  sum += f.resume
  sum
end
b = Task.new(name: "b", priority: 150) do
  sleep_ms 10
  puts "task b woke"
  :b
end
run_tasks(a, b)
p [a.value, b.value]

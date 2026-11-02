# checkpoint-test: suspend-after "before"
# Line numbers after a restore: exceptions raised after each suspend (at the
# top, in a Fiber, in a Task) print the same backtrace, with the same lines,
# as the run without checkpoints. The lines travel with the code (the DBG0
# section).
def run_tasks(*tasks)
  tasks.each(&:join)   # the program is a Task (checkpoint run --as-task)
rescue RuntimeError
  Task.run             # the program is at the root
end

def deep(n)
  raise ArgumentError, "deep #{n}" if n == 0
  deep(n - 1)
end

def show(e)
  puts "#{e.class}: #{e.message}"
  e.backtrace.each { |l| puts "  #{l}" }
end

puts "before"
Checkpoint.suspend("lines.ckpt")
begin
  deep(2)
rescue => e
  show(e)
end

f = Fiber.new do
  Checkpoint.suspend("lines.ckpt")
  begin
    deep(1)
  rescue => e
    show(e)
  end
  Fiber.yield :fiber
end
p f.resume

t = Task.new(name: "t", priority: 140) do
  Checkpoint.suspend("lines.ckpt")
  begin
    [3].each { |n| deep(n) }
  rescue => e
    show(e)
  end
  :task
end
run_tasks(t)
p t.value
p __LINE__

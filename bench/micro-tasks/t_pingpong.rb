# two tasks hand one value back and forth on two queues
N = 100_000
a = Task::Queue.new; b = Task::Queue.new
Task.new { i = 0; while i < N; a.push(i); b.pop; i += 1; end }
Task.new { i = 0; while i < N; b.push(a.pop); i += 1; end }
Task.run

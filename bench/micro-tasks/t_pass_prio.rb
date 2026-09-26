# 60 tasks at 6 priorities passing the CPU around
R = 30_000
60.times { |k| Task.new(priority: 100 + (k % 6) * 10) { i = 0; while i < R; Task.pass; i += 1; end } }
Task.run

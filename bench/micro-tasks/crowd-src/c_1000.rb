# 1000 tasks wait on queues nobody pushes to while two tasks play ping-pong 200000 times
idle = []
1000.times { q = Task::Queue.new; idle << q; Task.new { q.pop } }
a = Task::Queue.new; b = Task::Queue.new
Task.new { i = 0; while i < 200000; a.push(i); b.pop; i += 1; end; idle.each(&:close) }
Task.new { i = 0; while i < 200000; b.push(a.pop); i += 1; end }
Task.run

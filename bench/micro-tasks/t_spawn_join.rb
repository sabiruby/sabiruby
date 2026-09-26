# one task spawning a short task and joining it, again and again
R = 30_000
Task.new { i = 0; while i < R; t = Task.new { i }; t.join; i += 1; end }
Task.run

# 1000 tasks, each sleeping one tick and waking, in turn; 400 rounds apiece
N = 1000
R = 400
N.times { Task.new { i = 0; while i < R; sleep 0.001; i += 1; end } }
Task.run

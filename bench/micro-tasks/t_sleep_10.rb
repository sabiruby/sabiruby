# 10 tasks, each sleeping one tick and waking, in turn; 200000 rounds apiece
N = 10
R = 200000
N.times { Task.new { i = 0; while i < R; sleep 0.001; i += 1; end } }
Task.run

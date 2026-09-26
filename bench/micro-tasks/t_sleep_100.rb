# 100 tasks, each sleeping one tick and waking, in turn; 20000 rounds apiece
N = 100
R = 20000
N.times { Task.new { i = 0; while i < R; sleep 0.001; i += 1; end } }
Task.run

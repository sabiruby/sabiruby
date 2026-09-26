# Task.list and Task.stat with 1000 tasks in the queues (never run)
1000.times { Task.new { } }
i = 0
while i < 25000
  Task.list
  Task.stat
  i += 1
end

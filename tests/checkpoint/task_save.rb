# checkpoint-test: save-after "saving"
# Checkpoint.save inside a Task, while another Task waits on a queue: the
# saving program goes on; the restored one goes on from the same place and
# says so.
def run_tasks(*tasks)
  tasks.each(&:join)   # the program is a Task (checkpoint run --as-task)
rescue RuntimeError
  Task.run             # the program is at the root
end

q = Task::Queue.new
listener = Task.new(name: "listener", priority: 140) do
  v = q.pop
  puts "listener got #{v}"
  v
end
saver = Task.new(name: "saver", priority: 150) do
  puts "saving"
  r = Checkpoint.save("task_save.ckpt")
  puts "save returned #{r}"
  q.push(r)
  r
end
run_tasks(listener, saver)
p [listener.value, saver.value]

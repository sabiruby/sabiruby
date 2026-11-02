# checkpoint-test: suspend-after "waiting: 2"
# A producer and two consumers on a Task::Queue: the consumers wait on the
# queue (their order on it is part of the state), the producer pushes. The
# program suspends while both consumers wait, and in a consumer while the
# producer is ready, and the queue's items and waiters go along.
def run_tasks(*tasks)
  tasks.each(&:join)   # the program is a Task (checkpoint run --as-task)
rescue RuntimeError
  Task.run             # the program is at the root
end

q = Task::Queue.new
got = []
consumer = lambda do |name|
  Task.new(name: name, priority: 140) do
    loop do
      v = q.pop
      break if v == :stop
      got << "#{name}:#{v}"
      puts "consumer got #{v}"
      Checkpoint.suspend("task_queue.ckpt") if v == 2
    end
    name
  end
end
c1 = consumer.call("c1")
c2 = consumer.call("c2")
producer = Task.new(name: "producer", priority: 150) do
  puts "waiting: #{q.num_waiting}"
  Checkpoint.suspend("task_queue.ckpt")   # c1 and c2 wait on the queue
  q.push(1)
  q << 2
  q.push(3)
  q.push(:stop)
  Task.pass
  puts "left: #{q.size}"
  q.push(:stop)
  :pushed
end
run_tasks(c1, c2, producer)
p got
p [c1.value, c2.value, producer.value]
p q.size

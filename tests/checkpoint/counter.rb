# checkpoint-test: suspend-after "count 3"
# A counter that suspends twice and keeps counting on the next line.
i = 0
while i < 10
  i += 1
  puts "count #{i}"
  Checkpoint.suspend("counter.ckpt") if i == 3 || i == 7
end
puts "done #{i}"

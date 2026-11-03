# checkpoint-test: suspend-after "times 1"
# Stops inside blocks run by Ruby-defined iterators (Integer#times,
# Array#each, each_with_index): the iterator frames are permanent methods
# of mrblib and are bound by name and content hash on the other side.
sum = 0
3.times do |i|
  sum += i
  puts "times #{i}"
  Checkpoint.suspend("b.ckpt") if i == 1
end
puts "sum #{sum}"

words = %w(alpha beta gamma delta)
words.each_with_index do |w, idx|
  puts "#{idx}:#{w}"
  Checkpoint.suspend("b.ckpt") if w == "beta"
end

total = 0
[[1, 2], [3, 4], [5, 6]].each do |a, b|
  [a, b].each do |x|
    total += x
    Checkpoint.suspend("b.ckpt") if x == 4
  end
  puts "total #{total}"
end

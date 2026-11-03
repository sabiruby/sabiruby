# checkpoint-test: suspend-after "made [0, 1, 4]"
# Checkpoint.suspend / Checkpoint.save inside a block that a C function
# calls (Array#sort! via sort, Array.new with a block). The C frame between
# the root frame and the block cannot be carried, so the request is refused
# on the spot with Checkpoint::Error; the block rescues it and goes on.
# Back outside the C function, a suspend goes through.
n = 0
r = [3, 1, 2].sort do |a, b|
  n += 1
  puts "cmp #{n}"
  if n == 1
    begin
      Checkpoint.suspend("c.ckpt")
      puts "not reached"
    rescue Checkpoint::Error => e
      puts "refused: #{e.message}"
    end
  end
  a <=> b
end
puts "sorted #{r.inspect}"

made = Array.new(3) do |i|
  if i == 0
    begin
      Checkpoint.save("c.ckpt")
      puts "not reached"
    rescue Checkpoint::Error => e
      puts "refused: #{e.message}"
    end
  end
  puts "make #{i}"
  i * i
end
puts "made #{made.inspect}"
Checkpoint.suspend("c.ckpt")
puts "after #{n} #{made.sum}"

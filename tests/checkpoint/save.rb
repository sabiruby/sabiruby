# checkpoint-test: save-after "before save 6"
# Checkpoint.save writes the state and goes on; the restored program also
# goes on from the line after the save. The value of the call tells the two
# sides apart: :saved on the saving side, :resumed on the restored side.
# The save is made inside a block of a Ruby-defined iterator, two methods
# deep, with a closure over a local of the outer method.
def work(list)
  sum = 0
  seen = []
  add = ->(x) { seen << x }
  list.each do |x|
    sum += x
    add.call(x)
    if x == 3
      puts "before save #{sum}"
      side = Checkpoint.save("s.ckpt")
      puts "side #{side} resumed?=#{Checkpoint.resumed?}"
    end
  end
  [sum, seen]
end

def outer
  total, seen = work([1, 2, 3, 4, 5])
  "total #{total} seen #{seen.inspect}"
end

puts outer
puts "end"

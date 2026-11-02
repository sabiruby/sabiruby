# checkpoint-test: suspend-after "depth 3 before"
# Stops three methods deep, with arguments, locals and a keyword argument
# on the stack, and returns through all of them after the suspend.
def leaf(n, scale: 10)
  local = n * scale
  puts "depth 3 before"
  Checkpoint.suspend("m.ckpt")
  puts "depth 3 after local=#{local}"
  local + 1
end

def middle(a, b = 2, *rest)
  x = leaf(a + b, scale: 100)
  puts "depth 2 x=#{x} rest=#{rest.inspect}"
  x * 2
end

def top
  r = middle(1, 2, :p, :q)
  puts "depth 1 r=#{r}"
  r
end

puts "result #{top}"

# checkpoint-test: suspend-after "inner 1"
# checkpoint-test: sabiruby-skip-as-task "SabiRuby raises FiberError for Fiber#transfer inside a Task (its Task contexts run in a loop of their own), without any image"
# Fibers that resume Fibers: the root resumes outer, outer resumes inner. The
# program suspends in inner (outer and the root wait in Fiber#resume), in
# outer between the two, and at the root with both suspended. Fiber#transfer
# goes to a third Fiber and back.
inner = Fiber.new do |x|
  puts "inner #{x}"
  Checkpoint.suspend("fiber_nested.ckpt")   # inner runs, outer and the root wait
  y = Fiber.yield x * 10
  puts "inner got #{y}"
  :inner_end
end
outer = Fiber.new do |x|
  puts "outer #{x}"
  r = inner.resume(1)
  puts "outer got #{r}"
  Checkpoint.suspend("fiber_nested.ckpt")   # inner is suspended
  s = Fiber.yield r + x
  puts "outer got #{s}"
  t = inner.resume(s)
  puts "inner ended with #{t}"
  :outer_end
end
p outer.resume(5)
Checkpoint.suspend("fiber_nested.ckpt")     # both suspended
p outer.resume(7)
p [outer.alive?, inner.alive?]

back = Fiber.current
other = Fiber.new do |v|
  puts "transferred #{v}"
  Checkpoint.suspend("fiber_nested.ckpt")   # the root waits in Fiber#transfer
  back.transfer(v + 1)
end
p other.transfer(41)

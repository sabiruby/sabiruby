# checkpoint-test: suspend-after "in catch"
# checkpoint-test: sabiruby-refuse 1 "a frame runs Kernel#catch"
# checkpoint-test: sabiruby-refuse 2 "a Proc made from the C function F:Integer#+"
# Images that C mruby resumes but SabiRuby (the Rust mruby VM) refuses, with the
# reason: a frame of Kernel#catch (the writer's throw finds catch by its Proc on
# the call stack, SabiRuby's by a frame of its own), and a Method object whose
# Proc is a C function (SabiRuby's methods written in Rust are not Procs).
r = catch(:done) do
  puts "in catch"
  Checkpoint.suspend("x.ckpt")
  throw :done, 1
end
puts "caught #{r}"
plus = 3.method(:+)
Checkpoint.suspend("x.ckpt")
puts plus.call(4)

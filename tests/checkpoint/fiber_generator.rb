# checkpoint-test: suspend-after "fib 4 = 3"
# Generators: a Fiber that yields the Fibonacci numbers, and an Enumerator
# (whose #next runs a Fiber of mruby's own). The program suspends inside the
# generating Fiber, and at the top while both generators are half way.
fib = Fiber.new do
  a, b = 0, 1
  n = 0
  loop do
    Fiber.yield a
    n += 1
    Checkpoint.suspend("fiber_generator.ckpt") if n == 5   # inside the Fiber
    a, b = b, a + b
  end
end
8.times { |i| puts "fib #{i} = #{fib.resume}" }

squares = Enumerator.new do |y|
  i = 1
  loop do
    y << i * i
    i += 1
  end
end
3.times { puts "square #{squares.next}" }
Checkpoint.suspend("fiber_generator.ckpt")   # both generators are suspended
3.times { puts "square #{squares.next}" }
puts "fib then #{fib.resume}"
p fib.alive?

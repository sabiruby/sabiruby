# `method_missing` written in Ruby answers a call *in the frame the call was made in*, the
# way `send` does, rather than in a nested run loop. The reference does the same
# (`prepare_missing` in `src/vm.c` rewrites the frame OP_SEND already pushed), so everything
# here is checked against it: what the body receives (0, 1, 14, 15 and 20 positional
# arguments, keywords, a block), who answers (a private `method_missing`, `super` to the one
# above, `respond_to_missing?`), and what escapes it (an exception, its backtrace, and a
# `Fiber.yield` — which only a frame without a native boundary around it can do).
# expected-from: mruby 4.1.0-rc2

class Recorder
  def method_missing(name, *args, &blk)
    r = [name, args]
    r << blk.call(args.size) if blk
    r
  end

  def respond_to_missing?(name, include_private = false)
    name.to_s.start_with?("can_")
  end
end

r = Recorder.new
p r.none
p r.one(1)
p r.fourteen(*(1..14).to_a)
p r.fifteen(*(1..15).to_a)
p r.twenty(*(1..20).to_a)
p r.blocky(7) { |n| "block saw #{n}" }
p r.fifteen_and_block(*(1..15).to_a) { |n| n * 2 }

# `respond_to?` goes through `respond_to_missing?`, and `method` makes a Method out of it.
p [r.respond_to?(:can_fly), r.respond_to?(:cannot)]
p r.method(:can_fly).call(1, 2)

# Keywords arrive as a Hash, whatever the body's parameter list looks like.
class Keys
  def method_missing(name, *args, **kw)
    [name, args, kw]
  end
end
k = Keys.new
p k.plain(1, 2)
p k.kw(x: 1, y: 2)
p k.both(1, 2, x: 3)
p k.many(*(1..15).to_a, x: 1)
h = { a: 1, b: 2 }
p k.splat(**h)

# `super` inside `method_missing` finds the next `method_missing` up the chain, because the
# call is sent under the name `method_missing`, not under the name that was missing.
class Base
  def method_missing(name, *args)
    ["Base", name, args]
  end
end
class Middle < Base
  def method_missing(name, *args)
    ["Middle"] + super
  end
end
class Leaf < Middle
  def method_missing(name, *args)
    ["Leaf"] + super
  end
end
p Leaf.new.walk(1, 2)

# A private `method_missing` still answers: the fallback ignores visibility.
class Hidden
  def method_missing(name, *args)
    [:hidden, name]
  end
  private :method_missing
end
p Hidden.new.anything(1)
begin
  Hidden.new.send(:method_missing, :direct)
rescue NoMethodError => e
  p [:unreachable, e.message]
else
  p [:send_reaches_it]
end

# The basic `method_missing` (BasicObject's, written in C) still reports the error, and the
# NoMethodError names the method that was missing, not `method_missing`.
begin
  Object.new.not_here(1, 2)
rescue NoMethodError => e
  p [e.class, e.message, e.name, e.args]
end

# An exception raised inside `method_missing` propagates like any other, and its backtrace
# has the `method_missing` frame in it.
class Raiser
  def method_missing(name, *args)
    raise ArgumentError, "no #{name}"
  end
end
begin
  Raiser.new.boom(1)
rescue ArgumentError => e
  p [e.class, e.message]
  puts e.backtrace.map { |l| l.split("/").last }.join("\n")
end

# `raise` from a block the body yielded to also gets out.
class Yielder
  def method_missing(name, *args, &blk)
    blk.call
  end
end
begin
  Yielder.new.go { raise "from the block" }
rescue RuntimeError => e
  p e.message
end

# The body is an ordinary frame, so a Fiber can be suspended from inside it.
fib = Fiber.new do
  o = Object.new
  def o.method_missing(name, *args)
    got = Fiber.yield([:asked, name, args])
    [:resumed_with, got]
  end
  o.question(6, 7)
end
p fib.resume
p fib.resume(:answer)

# ... and so can one that is nested two `method_missing` deep.
fib2 = Fiber.new do
  a = Object.new
  def a.method_missing(name, *args)
    Fiber.yield([:outer, name])
    b = Object.new
    def b.method_missing(n2, *a2)
      Fiber.yield([:inner, n2])
      :inner_done
    end
    [:outer_done, b.second]
  end
  a.first
end
p fib2.resume
p fib2.resume
p fib2.resume

# `return` from inside the body returns from `method_missing`, not from the caller.
class Returner
  def method_missing(name, *args)
    return :early if args.empty?
    :late
  end
end
def call_returner(o)
  v = o.whatever
  [:after, v]
end
p call_returner(Returner.new)

# A block made in the caller can `break` out of a method called from inside the body,
# because there is no native frame between them any more.
class Breaker
  def method_missing(name, *args, &blk)
    [1, 2, 3].each { |i| blk.call(i) }
    :not_broken
  end
end
def try_break(o)
  o.loopy { |i| break :broke_at_1 if i == 1 }
end
p try_break(Breaker.new)

# `__method__` inside the body is `method_missing`.
class Named
  def method_missing(name, *args)
    [__method__, name]
  end
end
p Named.new.whoami

# Deep recursion through `method_missing` still ends in a SystemStackError rather than
# running away.
class Recur
  def method_missing(name, *args)
    send(:"#{name}x")
  end
end
begin
  Recur.new.a
rescue SystemStackError => e
  p [:stack, e.class]
rescue NoMethodError => e
  p [:nomethod]
end

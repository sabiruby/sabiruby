# checkpoint-test: suspend-after "patched"
# Changes to classes that mrb_open made: methods added to String and
# Integer, an alias of a C method in Array, a top-level method (a private
# method of Object), an ivar on main, globals, a new constant in Object.
class String
  def shout = upcase + "!"
end

class Integer
  def double = self * 2
end

class Array
  alias_method :count_of, :size
end

def helper(x) = "helper(#{x})"

@main_ivar = :on_main
$seen = ["g1"]
$level = 3
LIMIT = 42
puts "patched"
Checkpoint.suspend("bp.ckpt")
$seen << "g2"
puts "abc".shout, 21.double, [1, 2, 3].count_of, helper(1)
puts @main_ivar.inspect, $seen.inspect, $level, LIMIT
puts String.instance_method(:shout).owner, 7.respond_to?(:double)

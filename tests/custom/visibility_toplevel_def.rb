# A `def` written at the top level is a private method of Object in the reference, because the
# base frame of a context starts out private (src/vm.c stack_init: `c->ci->vis = 1`) and every
# frame pushed onto it starts public (cipush). leftovers-plan.md item 10.
# Expectation: the reference mruby's own output.

def top_m; "top_m"; end

puts "listed private: #{Object.private_instance_methods(false).include?(:top_m)}"
puts "listed public: #{Object.public_instance_methods(false).include?(:top_m)}"

# an implicit-self call is not checked
puts "implicit: #{top_m}"
# `self.m` is an SSEND, which the reference does not check either (as CRuby since 2.7)
puts "self: #{self.top_m}"
# an explicit receiver is
begin
  Object.new.top_m
rescue NoMethodError => e
  puts "receiver: #{e.message}"
end
# `send` ignores visibility, `public_send` does not
puts "send: #{Object.new.send(:top_m)}"
begin
  Object.new.public_send(:top_m)
rescue NoMethodError => e
  puts "public_send: #{e.message}"
end

# a block written at the top level shares the frame's visibility through its env
[1].each { def in_block; end }
puts "in a block: #{Object.private_instance_methods(false).include?(:in_block)}"

# so does a block that ran in another context
Fiber.new { def in_fiber; end }.resume
puts "in a fiber: #{Object.private_instance_methods(false).include?(:in_fiber)}"

# a `def` inside a method body is on the method's frame, which is public
def outer; def inner; end; end
outer
puts "inside a def: #{Object.private_instance_methods(false).include?(:inner)}"

# ... and so is a class body, an `Object.class_eval`, and a singleton
class Klass; def in_class; end; end
puts "in a class: #{Klass.private_instance_methods(false).include?(:in_class)}"
Object.class_eval { def in_class_eval; end }
puts "in class_eval: #{Object.private_instance_methods(false).include?(:in_class_eval)}"
o = Object.new
o.instance_eval { def on_singleton; end }
puts "on a singleton: #{o.singleton_class.private_instance_methods(false).include?(:on_singleton)}"

# `Module#define_method` writes the visibility itself (MRB_METHOD_PUBLIC_FL), so a bare
# `private` or `module_function` in the scope does not reach it
class Vis
  private
  define_method(:by_define_method) { 1 }
  def by_def; 2; end
end
puts "define_method under private: #{Vis.private_instance_methods(false).include?(:by_define_method)}"
puts "def under private: #{Vis.private_instance_methods(false).include?(:by_def)}"

# `private` with no arguments inside a class body, then a name again
class Vis2
  def a; 1; end
  private
  def b; 2; end
  public
  def c; 3; end
  private :c
end
puts "Vis2 private: #{Vis2.private_instance_methods(false).sort.inspect}"
puts "Vis2 public: #{Vis2.public_instance_methods(false).sort.inspect}"

# module_function with no arguments
module MF
  module_function
  def mf1; "mf1"; end
end
puts "MF#mf1 private: #{MF.private_instance_methods(false).include?(:mf1)}"
puts "MF.mf1: #{MF.mf1}"

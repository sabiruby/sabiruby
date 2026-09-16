# What follows from the methods being private: respond_to? (with and without include_all),
# send vs public_send, Object#method, the instance_methods listings, and the NoMethodError text.
# leftovers-plan.md item 10. Expectation: the reference mruby's own output.

o = Object.new

# respond_to? is false for a private method unless the second argument is true
puts "respond_to?(:puts): #{o.respond_to?(:puts)}"
puts "respond_to?(:puts, true): #{o.respond_to?(:puts, true)}"
puts "respond_to?(:puts, false): #{o.respond_to?(:puts, false)}"
puts "respond_to?(:inspect): #{o.respond_to?(:inspect)}"
puts "respond_to?(:initialize): #{o.respond_to?(:initialize)}"
puts "respond_to?(:initialize, true): #{o.respond_to?(:initialize, true)}"
puts "Array.respond_to?(:private): #{Array.respond_to?(:private)}"
puts "Array.respond_to?(:private, true): #{Array.respond_to?(:private, true)}"

# send reaches a private method, public_send does not
puts "send(:format): #{o.send(:format, '%02d', 5)}"
begin
  o.public_send(:format, '%02d', 5)
rescue NoMethodError => e
  puts "public_send(:format): #{e.message}"
end

# a protected method is refused by public_send too, with its own word
class Prot
  def call_other(x); x.prot; end
  protected
  def prot; "prot"; end
end
p1 = Prot.new
puts "protected through a peer: #{Prot.new.call_other(p1)}"
begin
  p1.prot
rescue NoMethodError => e
  puts "protected with a receiver: #{e.message}"
end
begin
  p1.public_send(:prot)
rescue NoMethodError => e
  puts "protected public_send: #{e.message}"
end
puts "protected send: #{p1.send(:prot)}"

# Object#method takes a private method; Method#call runs it
m = o.method(:format)
puts "method(:format): #{m.call('%03d', 7)}"
puts "method owner: #{m.owner}"

# the listings
class Listed
  def pub; end
  private
  def priv; end
  protected
  def prot; end
end
puts "instance_methods: #{Listed.instance_methods(false).sort.inspect}"
puts "public_instance_methods: #{Listed.public_instance_methods(false).sort.inspect}"
puts "private_instance_methods: #{Listed.private_instance_methods(false).sort.inspect}"
puts "protected_instance_methods: #{Listed.protected_instance_methods(false).sort.inspect}"
l = Listed.new
puts "methods includes prot: #{l.methods.include?(:prot)}"
puts "methods includes priv: #{l.methods.include?(:priv)}"
puts "private_methods includes priv: #{l.private_methods.include?(:priv)}"

# Kernel's own private methods are reachable as module functions on Kernel
puts "Kernel.format: #{Kernel.format('%x', 255)}"
begin
  Kernel.new
rescue NoMethodError => e
  puts "Kernel.new: #{e.class}"
end

# respond_to_missing? is asked only when the method is not found at all
class RTM
  def respond_to_missing?(name, include_all)
    name == :ghost || super
  end
end
r = RTM.new
puts "ghost: #{r.respond_to?(:ghost)}"
puts "ghost include_all: #{r.respond_to?(:ghost, true)}"
puts "nothing: #{r.respond_to?(:nothing)}"
puts "respond_to_missing? is private: #{RTM.private_instance_methods(false).include?(:respond_to_missing?)}"

# The natives on the way to a raise are in `Exception#backtrace`, located at the nearest Ruby
# frame below them, as the reference's C frames are (`pack_backtrace`, src/backtrace.c):
# the native that raised (`Integer`, `center`, `__fetch`), a native whose block raised
# (`sort!`, `gsub`, `each_object`), and a native loop frame (`index { }`, `Array.new(n) { }`,
# but not `catch { }`, which is bytecode in the reference). `raise` itself is not named, and a
# method of mrblib has no debug info and is left out, in both. `caller` sees the same natives.
# expected-from: mruby 4.1.0-rc2, except where SabiRuby runs a native's block in a frame of its
# own (`Class.new { }`, `Module.new { }`, `Struct.new { }`, `delete(x) { }`, `instance_exec`),
# where `eval`'s frame has no method name, and where a native called by a native
# (`Method#call` of `+`) is named by the one the SEND called (`docs/design/exceptions.md`).

def t
  yield
rescue => e
  p e.backtrace
end

def foo
  raise "x"
end
t { foo }
t { Integer("zz") }
def bar
  [1].fetch(5)
end
t { bar }
t { [1, 2].each { |x| nil.zork } }
def baz
  [1, 2].map { |x| raise ArgumentError, "q" }
end
t { baz }
t { 1 / 0 }
t { [3, 1].sort { |a, b| raise "s" } }
t { "abc".center }
t { [1].index { raise "i" } }
t { [1].rindex { raise "r" } }
t { Array.new(2) { raise "a" } }
t { catch { raise "c" } }
t { catch(:x) { [1].index { raise "ci" } } }
t { Hash.new { raise "h" }[1] }
t { "a".gsub("a") { raise "g" } }
t { ObjectSpace.each_object { raise "os" } }
t { [1].each_with_index { raise "ewi" } }
def deep
  [1].map { |v| Integer("q") }
end
t { deep }
x = nil
[3, 1].sort { |a, b| x = caller; 0 }
p x
def c1; caller; end
p c1
t { Class.new { raise "k" } }
t { Module.new { raise "m" } }
t { Struct.new(:a) { raise "s" } }
t { [1].delete(5) { raise "d" } }
t { [1].instance_exec { raise "ix" } }
t { eval("raise 'ev'") }
t { 1.method(:+).call(nil) }

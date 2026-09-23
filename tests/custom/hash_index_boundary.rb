# A Hash must behave the same whichever representation it has inside. SabiRuby keeps the
# entries in insertion order and, once there are more than 16 of them, builds an index from
# the cached hash code to the entry (the reference switches its RHash from "array" to a hash
# table at the same size). This walks the two sides of that boundary and the ways an index
# can go stale: deleting below it again, rehash, replacing every entry, and keys that answer
# the same `hash` but are not `eql?`.
# expected-from: mruby 4.1.0-rc2 (this is about a representation Ruby cannot see)

def build(n)
  h = {}
  n.times { |i| h["k#{i}"] = i }
  h
end

[15, 16, 17, 40].each do |n|
  h = build(n)
  p [n, h.size, h.keys.first, h.keys.last, h["k0"], h["k#{n - 1}"], h["nope"]]
  p h.keys == (0...n).map { |i| "k#{i}" }
  p h.to_a.first == ["k0", 0]
end

# deleting back below the boundary, then looking up and adding again
h = build(20)
5.times { |i| h.delete("k#{i}") }
p [h.size, h["k4"], h["k5"], h.keys.first]
10.times { |i| h.delete("k#{i + 5}") }
p [h.size, h.keys.first, h.keys.last]
h["late"] = :late
p [h.size, h["late"], h.keys.last]

# delete from the middle of a big hash: the entries after it move
h = build(30)
h.delete("k10")
p [h.size, h["k9"], h["k10"], h["k11"], h["k29"], h.keys[10]]

# Hash#shift takes the first entry away
h = build(20)
p h.shift
p [h.size, h["k0"], h["k1"], h.keys.first]

# keys that collide: the same hash, not eql?
class Same
  def initialize(n) = @n = n
  def hash = 42
  def eql?(o) = o.is_a?(Same) && o.n == @n
  def ==(o) = eql?(o)
  attr_reader :n
end
h = {}
25.times { |i| h[Same.new(i)] = i }
p h.size
p [h[Same.new(0)], h[Same.new(24)], h[Same.new(25)]]
h.delete(Same.new(3))
p [h.size, h[Same.new(3)], h[Same.new(4)]]

# rehash after a key was mutated under the hash
k = [1]
h = build(20)
h[k] = :ary
k << 2
p h[[1, 2]]
h.rehash
p [h[[1, 2]], h[[1]], h.size, h["k0"]]

# wholesale replacements: every entry and every cached hash code is thrown away
a = build(20)
b = build(3)
b.replace(a)
p [b.size, b["k19"], b.keys.first]
c = a.dup
c["extra"] = 1
p [a.size, c.size, c["k19"], c["extra"]]
d = build(18).merge(build(20))
p [d.size, d["k19"], d.keys.first, d.keys.last]
e = build(20)
e.clear
p [e.size, e["k0"]]
e["after"] = 1
p [e.size, e["after"]]

# nil values and compact!
f = build(20)
%w[k3 k7 k19].each { |x| f[x] = nil }
p f.size
f.compact!
p [f.size, f["k3"], f["k4"], f["k19"], f.keys.last]

# frozen string keys are copied in, so a later change to the key does not move the entry
s = "mutable"
g = build(20)
g[s] = :v
s << "!"
p [g["mutable"], g["mutable!"], g.size]

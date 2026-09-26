# A Set prints its elements under `p` as under `inspect` (it printed `Set[...]`: the recursion
# mark `p` leaves for the receiver was read as the Set containing itself), a Struct likewise,
# and a Set that does contain itself still prints `Set[...]` inside.
# mruby-set's khash asks `eql?` of the element already in the set (kset_equal_value(keys[k],
# key)), so a String there answers natively and an element whose `eql?` raises is added to
# Set["k0"]; the same object is found by identity (mrb_eql); an `eql?` that does run and raises
# is raised. An element whose `hash` raises counts as hash 0 and is added
# (mrbgems/mruby-set/src/set.c, kset_hash_value / kset_equal_value).
# expected-from: mruby 4.1.0-rc2

S = Struct.new(:a)
p S.new(1)
p Set.new([1, 2])
p [Set.new([1])]
p({k: Set[1]})
p Set[Set[1], S.new(Set[2])]
s = Set.new([1]); s << s
p s
puts s.inspect
t = S.new(nil); t.a = t
p t
p Set.new

class Bad
  def eql?(o) raise ArgumentError, "boom" end
  def hash; "k0".hash end
end
class BadHash
  def hash; raise ArgumentError, "no hash" end
end
def t(name)
  r = yield
  puts "#{name}: #{r.inspect}"
rescue => e
  puts "#{name}: #{e.class}: #{e.message}"
end
b = Bad.new
t(:add) { s = Set.new(["k0"]); s.add(b); [s.size, s.include?(b)] }
t(:add?) { s = Set.new(["k0"]); r = s.add?(b); [r.class, s.size] }
t(:add_twice) { s = Set.new(["k0"]); s.add(b); s.add(b); s.size }
t(:new) { Set.new(["k0", b]).size }
t(:merge) { s = Set.new(["k0"]); s.merge([b]); s.size }
t(:or) { (Set.new(["k0"]) | Set.new([b])).size }
t(:include) { Set.new(["k0"]).include?(b) }
t(:include_other) { Set.new([b]).include?(Bad.new) }
t(:delete) { Set.new(["k0"]).delete(b).size }
t(:delete_self) { Set.new(["k0", b]).delete(b).size }
t(:hash_add) { s = Set.new(["k0"]); s.add(BadHash.new); s.size }
t(:hash_include) { Set.new(["k0"]).include?(BadHash.new) }
t(:hash_delete) { Set.new(["k0"]).delete(BadHash.new).size }
t(:hash_new) { Set.new([BadHash.new]).size }
t(:hash_self) { x = BadHash.new; s = Set.new([x]); [s.include?(x), s.delete(x).size] }
# a Bad already in the set is the one asked `eql?`, and what it raises is raised
t(:inc) { Set.new([Bad.new]).include?(Bad.new) }
t(:inc2) { s = Set.new([Bad.new]); x = s.include?(Bad.new); [x, 1] }
t(:add) { s = Set.new([Bad.new]); s.add(Bad.new); s.size }
t(:del) { s = Set.new([Bad.new]); s.delete(Bad.new); s.size }
t(:new2) { Set.new([Bad.new, Bad.new]).size }
t(:hinc) { Set.new(["a"]).include?(BadHash.new) }
t(:hinc2) { s = Set.new(["a"]); x = s.include?(BadHash.new); [x, 1] }
t(:hadd) { s = Set.new(["a"]); s.add(BadHash.new); s.size }
t(:hadd2) { s = Set.new; s.add(BadHash.new); s.size }
t(:hlit) { Set[BadHash.new].size }

# `Enumerable#hash` (mrblib/enum.rb) asks `__method_recursive?(:hash)` (src/kernel.c,
# `mrb_obj_method_recursive_p`) and folds with `Enumerable.__update_hash` (src/enum.c); neither
# existed here, so the hash of a Struct or of any class including Enumerable raised NoMethodError.
# The numbers the fold gives are compared only among themselves (an element's `hash` is the
# VM's own), except `__update_hash` on literal numbers, which is the reference's formula.
# Also `Array#index` / `#rindex` with neither an argument nor a block answer an Enumerator
# (`mrb_ary_index_m`), rather than calling a missing block.
# expected-from: mruby 4.1.0-rc2
S = Struct.new(:a, :b)
p S.new(1, 2).hash.class
p S.new(1, 2).hash == S.new(1, 2).hash
p S.new(1, 2).hash == S.new(2, 1).hash
t = S.new(nil, 1); t.a = t
p t.hash.class
class E; include Enumerable; def each; yield 1; yield 2; end; end
p E.new.hash.class, E.new.hash == E.new.hash
p({S.new(1, 2) => :x}[S.new(1, 2)])
p Set[S.new(1, 2)].include?(S.new(1, 2))
p Enumerable.__update_hash(12347, 0, 5) == (12347 ^ 5)
p Enumerable.__update_hash(0, 17, 1)
p Enumerable.__update_hash(0, 15, -1)
begin; Enumerable.__update_hash(0, 0, "x"); rescue TypeError => e; p e.message; end
p 1.__method_recursive?(:hash)
class R; def hash; __method_recursive?(:hash) ? 0 : [self].hash + 1; end; end
p R.new.hash.class
e = [3, 1, 3].index
p e.class, e.each { |x| x == 1 }
p [3, 1, 3].rindex.each { |x| x == 3 }

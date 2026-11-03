# checkpoint-test: suspend-after "filled"
# Hashes are rebuilt key by key on the other side (the hash values of keys
# are recomputed there): String, Symbol, Array, Float and user-object keys,
# a default value, a default proc, ranges as values.
class Key
  attr_reader :id
  def initialize(id) = @id = id
  def hash = id.hash
  def eql?(o) = o.is_a?(Key) && o.id == id
end

h = { "str" => 1, sym: 2, [1, 2] => 3, 1.5 => 4, Key.new(7) => 5, nil => 6 }
counts = Hash.new(0)
%w(a b a c a).each { |w| counts[w] += 1 }
memo = Hash.new { |hh, k| hh[k] = k * k }
memo[3]
ranges = { small: (1..3), open: (5...), excl: (1...4) }
nested = { list: [1, { deep: "x" }], again: h }
puts "filled"
Checkpoint.suspend("h.ckpt")
puts h["str"], h[:sym], h[[1, 2]], h[1.5], h[Key.new(7)], h[nil]
puts h.keys.map(&:class).inspect
puts counts.inspect, counts["zzz"], memo[4], memo.inspect
puts ranges[:small].to_a.inspect, ranges[:open].first(2).inspect, ranges[:excl].size
puts nested[:list][1][:deep], nested[:again].equal?(h)
h["new"] = 9
puts h.size, h.frozen?, "lit".frozen?

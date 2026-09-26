# What a key's `eql?` (or `hash`) raises during a Hash lookup reaches the caller: `[]`, `key?`,
# `fetch`, `delete`, `dig`, `values_at`, `slice`, `except`, `==`, `eql?`, `merge!`. And an
# `eql?` that adds or removes entries of the Hash being searched makes the lookup raise
# RuntimeError "hash modified" (mruby 4.1.0-rc2's H_CHECK_MODIFIED, GHSA-2778-fvwg-5m8w), in a
# small Hash and in one past the 16 entries where SabiRuby builds its index. The keys answer
# the `hash` of a stored key, so `eql?` is reached in SabiRuby too (it compares hash codes
# first; the reference's small Hash calls `eql?` on every entry). The same object is the same
# key without asking `eql?` (mrb_eql answers identity first), so such a key still finds itself.
# A Set asks the other way round (tests/custom/set_elements.rb).
# expected-from: mruby 4.1.0-rc2, except "hash raises 3" (and its key?): the reference's small
# Hash does not call `hash`, so it answers nil/false; SabiRuby raises there as the reference
# does past 16 entries (and as CRuby does at any size).

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

h = {"k0" => 0}
b = Bad.new
t(:aref) { h[b] }
t(:key?) { h.key?(b) }
t(:include?) { h.include?(b) }
t(:member?) { h.member?(b) }
t(:has_key?) { h.has_key?(b) }
t(:fetch) { h.fetch(b, :d) }
t(:delete) { h.delete(b) }
t(:aset) { h[b] = 1 }
t(:store) { h.store(b, 1) }
t(:dig) { h.dig(b) }
t(:dig_nested) { {x: h}.dig(:x, b) }
t(:values_at) { h.values_at(b) }
t(:slice) { h.slice(b) }
t(:except) { h.except(b) }
t(:merge!) { {"k0" => 1}.merge!({b => 2}) { |k, x, y| x + y } }
t(:eq) { {b => 1} == {"k0" => 1} }
t(:eql?) { {b => 1}.eql?({"k0" => 1}) }
t(:eq_other_side) { {"k0" => 1} == {b => 1} }
p h

# Keys of a class of their own, so that the reference's hash table (past 16 entries) also
# compares hash codes it got from `hash` and reaches `eql?` (its code for a String key is not
# what `String#hash` answers, so a key answering `"k0".hash` would not meet "k0" there).
class Key
  attr_reader :i
  def initialize(i) @i = i end
  def hash; @i end
  def eql?(o) o.is_a?(Key) && o.i == @i end
end
class Evil
  def initialize(h, del) @h = h; @del = del end
  def eql?(o) @del.each { |k| @h.delete(k) }; false end
  def hash; 0 end
end
class Grow
  def initialize(h) @h = h end
  def eql?(o) @h[Key.new(-1)] = 1; false end
  def hash; 0 end
end
[3, 20].each do |n|
  [:aref, :key?, :aset, :delete, :fetch].each do |m|
    keys = (0...n).map { |i| Key.new(i) }
    h2 = {}
    keys.each { |k| h2[k] = k.i }
    e = Evil.new(h2, [keys[1], keys[2]])
    t("evil #{n} #{m}") do
      case m
      when :aref then h2[e]
      when :key? then h2.key?(e)
      when :aset then h2[e] = 9
      when :delete then h2.delete(e)
      when :fetch then h2.fetch(e, :d)
      end
    end
    p h2.size
  end
  h3 = {}
  n.times { |i| h3[Key.new(i)] = i }
  t("grow #{n}") { h3[Grow.new(h3)] }
  p h3.size
  # a key whose `hash` raises: the reference's small Hash never asks for it (its "array"
  # form compares with `eql?` only), so it answers nil there; SabiRuby hashes every key
  h5 = {}
  n.times { |i| h5["k#{i}"] = i }
  t("hash raises #{n}") { h5[BadHash.new] }
  t("hash raises #{n} key?") { h5.key?(BadHash.new) }
end

# an eql? that changes nothing still answers
class Good
  def eql?(o) o == "k0" end
  def hash; "k0".hash end
end
h4 = {"k0" => 5}
p h4[Good.new], h4.key?(Good.new), h4.delete(Good.new), h4


# identity before eql?
class Never
  def eql?(o) false end
  def hash; 1 end
end
n = Never.new
h6 = {n => 1}
p h6[n], h6.key?(n)
h7 = {}
h7[b] = 1
h7[b] = 2
p h7.size, h7[b], h7.delete(b), h7.size

# A string class_eval/instance_eval must not change where a later `def` in the
# caller's frame lands. mruby 4.1.0-rc2 (and -rc) gets this wrong: the string's proc shares
# the caller's env and MRB_PROC_SET_TARGET_CLASS writes the receiver into that
# shared env, so `def g` below is added to K (reference prints "true\nfalse").
# Fixed on mruby master by 300cc9532 (2026-09-06, "give a string class_eval a scope of
# its own instead of the caller's env"); no tag has it yet, 4.1.0-rc2 included (.rc.out).
# expected-from: CRuby 3.2 (the master fix targets the same behaviour; not run)
class K; end
def f
  K.class_eval "def hi; :hi; end"
  def g; 2; end
end
f
p K.new.respond_to?(:g, true), Object.new.respond_to?(:g, true)

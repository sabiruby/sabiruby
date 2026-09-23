# From inside a method, an eval string cannot see a top-level local: the name
# is unknown and becomes a NameError (NoMethodError in mruby, a NameError
# subclass). mruby 4.1.0-rc2 (and -rc) instead fails at compile time with SyntaxError
# ("generator error, Can't find local variables"): the parser is told about
# every scope up to the C frame (mrc_pm_options_init) while codegen's
# search_upvar stops at the method's SCOPE proc, so the two disagree.
# SabiRuby hands the same table (stopping at the scope) to both.
# expected-from: CRuby 3.2
x = 1
def m
  eval "x"
end
begin
  m
rescue NameError => e
  p e.name          # :x (a missing `eval` itself would give :eval)
end
p x

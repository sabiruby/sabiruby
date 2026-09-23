# Kernel#eval sees and writes the caller's locals, also from blocks made inside
# the string; locals made inside the string do not leak out; a string
# class_eval defines on the receiver; a syntax error is a SyntaxError.
# Semantics shared by CRuby 3.2 and the reference mruby. (Numbered parameters
# seen from eval are mruby-only -- CRuby raises NameError -- and are covered by
# mruby's own mruby-eval/test/eval.rb.)
# expected-from: reference mruby 4.1.0-rc2 (same as CRuby 3.2)
def f
  a = 10
  eval "a += 1"
  b = eval "lambda { a * 2 }.call"
  eval "c = 1"
  [a, b, defined?(c)]
end
p f
class K; end
K.class_eval "def hi; :hi; end"
p K.new.hi
begin
  eval "1 +"
rescue SyntaxError => e
  p e.class
end

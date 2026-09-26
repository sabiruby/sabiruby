class C; def f(x) = x + 1; end
o = C.new
i = 0; s = 0
while i < N
  s = o.public_send(:f, s)
  i += 1
end

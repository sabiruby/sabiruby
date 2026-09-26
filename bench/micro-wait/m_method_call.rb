def f(x) = x + 1
m = method(:f)
i = 0; s = 0
while i < N
  s = m.call(s)
  i += 1
end

def f(x) = x + 1
i = 0; s = 0
while i < N
  s = send(:f, s)
  i += 1
end

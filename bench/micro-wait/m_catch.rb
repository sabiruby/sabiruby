i = 0; s = 0
while i < N
  s += catch(:t) { throw :t, 1 }
  s += catch { 1 }
  i += 1
end

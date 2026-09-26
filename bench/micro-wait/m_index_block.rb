a = (0...50).to_a
i = 0; s = 0
while i < N
  s += a.index { |x| x == 40 }
  i += 1
end

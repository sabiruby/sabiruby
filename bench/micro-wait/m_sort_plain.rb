a = (0...50).map { |x| (x * 7919) % 50 }
i = 0
while i < N
  a.sort
  i += 1
end

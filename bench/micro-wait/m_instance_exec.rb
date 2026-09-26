o = Object.new
i = 0; s = 0
while i < N
  s += o.instance_exec(1) { |x| x }
  i += 1
end

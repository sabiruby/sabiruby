o = Object.new
i = 0; s = 0
while i < N
  s += o.instance_eval { 1 }
  i += 1
end

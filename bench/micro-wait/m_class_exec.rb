c = Class.new
i = 0; s = 0
while i < N
  s += c.class_exec { 1 }
  i += 1
end

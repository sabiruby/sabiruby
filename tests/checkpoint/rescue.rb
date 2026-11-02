# checkpoint-test: suspend-after "rescued boom"
# Stops inside a rescue clause (the exception object is in a register),
# then an ensure clause and a retry run on the other side.
class AppError < StandardError
  def initialize(msg = "app failed") = super
end

attempts = 0
begin
  attempts += 1
  raise AppError, "boom" if attempts == 1
  raise ArgumentError if attempts == 2
  puts "ok after #{attempts}"
rescue AppError => e
  puts "rescued #{e.message}"
  Checkpoint.suspend("r.ckpt")
  puts "class #{e.class} #{e.is_a?(StandardError)}"
  retry
rescue ArgumentError => e
  puts "second #{e.class}"
  retry
ensure
  puts "ensure #{attempts}"
end

def risky
  yield
rescue => e
  "caught #{e.message}"
ensure
  puts "risky ensure"
end
puts risky { Checkpoint.suspend("r.ckpt"); raise "late" }

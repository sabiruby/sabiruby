# checkpoint-test: suspend-after "before"
# Line numbers of mrblib after a restore: the suspend is in a block that
# Array#each (mrblib/array.rb) calls, so the image has the frame of Array#each
# and carries its irep with its lines (DBG0). The exception raised after the
# resume prints that frame's file and line, as the run without checkpoints
# does: on this build from its own mrblib, on SabiRuby (another mrblib, built
# without lines) from the lines in the image.
def deep(n)
  raise ArgumentError, "deep #{n}" if n == 0
  deep(n - 1)
end

puts "before"
[2].each do |n|
  Checkpoint.suspend("lines_mrblib.ckpt")
  begin
    deep(n)
  rescue => e
    puts "#{e.class}: #{e.message}"
    e.backtrace.each { |l| puts "  #{l}" }
  end
end
p __LINE__

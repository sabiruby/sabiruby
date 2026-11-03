# checkpoint-test: suspend-after "box 41"
# C data (RData) travels only through the side table: Checkpoint::Box registers
# a dump and a restore callback for its mrb_data_type.
b = Checkpoint::Box.new(41)
puts "box #{b.value}"
Checkpoint.suspend("d.ckpt")
b.value += 1
puts "box #{b.value} #{b.class}"

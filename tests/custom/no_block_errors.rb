# The errors of methods that take a block or a string, called with neither (or with the wrong
# arguments), as mruby 4.1.0-rc2 words them:
# - instance_eval / class_eval / module_eval with nothing: `mrb_argnum_error(argc, 1, 3)`; with
#   too many: "expected 0..3"; a source that is not a String: "%Y cannot be converted to String"
#   (eval and Binding#eval too).
# - catch without a block: its body is `r2.call(r1)`, so NoMethodError "for NilClass" (%T).
# - define_method / define_singleton_method / Proc.new without a block: "no block given"
#   (src/class.c, `mrb_get_args` "&!"); lambda keeps "tried to create Proc object without a block".
# expected-from: mruby 4.1.0-rc2
def t(name)
  r = yield
  puts "#{name}: #{r.inspect}"
rescue => e
  puts "#{name}: #{e.class}: #{e.message}"
end
t(:instance_eval) { 1.instance_eval }
t(:class_eval) { Class.new.class_eval }
t(:module_eval) { Module.new.module_eval }
t(:instance_eval_4) { 1.instance_eval("1", "f", 1, 2) }
t(:class_eval_4) { Class.new.class_eval("1", "f", 1, 2) }
t(:instance_eval_str_blk) { 1.instance_eval("1") { } }
t(:instance_eval_int) { 1.instance_eval(1, 2) }
t(:class_eval_int) { Class.new.class_eval(1) }
t(:instance_eval_nil) { 1.instance_eval(nil) }
t(:eval_int) { eval(1) }
t(:eval_nil) { eval(nil) }
t(:binding_eval_int) { binding.eval(1) }
t(:instance_eval_ok) { 1.instance_eval("self + 1") }
t(:class_eval_ok) { Class.new.class_eval("1 + 1") }
t(:catch) { catch }
t(:catch_tag) { catch(:x) }
t(:catch_ok) { catch(:x) { throw :x, 5 } }
t(:nil_call) { nil.call }
t(:define_method) { Class.new { define_method(:x) } }
t(:define_method_int) { Class.new { define_method(:x, 1) } }
t(:define_singleton_method) { Object.new.define_singleton_method(:x) }
t(:proc_new) { Proc.new }
t(:proc) { proc }
t(:lambda) { lambda }

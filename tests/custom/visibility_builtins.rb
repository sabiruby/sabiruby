# The 50 methods docs/verification/coverage.md listed under "On both sides, with a different
# visibility": private in the reference, public here until leftovers-plan.md item 10. Expectation:
# the reference mruby's own output. Each line is the name and what the reference answers for it,
# so a regression names the method. Regexp's two are in visibility_builtins_regexp.rb (a build
# without the feature has no Regexp at all).
# `Module#private_method_defined?` is a SabiRuby addition, so the listings are asked for
# instead: `*_instance_methods(false)` is what tools/coverage.rb asks too.
def show(mod, name)
  m = if mod.private_instance_methods(false).include?(name) then "private"
      elsif mod.protected_instance_methods(false).include?(name) then "protected"
      elsif mod.public_instance_methods(false).include?(name) then "public"
      else "not defined here"
      end
  puts "#{mod}##{name}: #{m}"
end

[[Array, :initialize], [Array, :initialize_copy],
 [BasicObject, :initialize], [BasicObject, :method_missing],
 [BasicObject, :singleton_method_added], [BasicObject, :singleton_method_removed],
 [BasicObject, :singleton_method_undefined],
 [Binding, :initialize_copy], [Class, :inherited], [Data, :initialize],
 [Exception, :initialize], [Fiber, :initialize],
 [Hash, :initialize], [Hash, :initialize_copy],
 [Kernel, :Complex], [Kernel, :Rational],
 [Kernel, :__defined_const?], [Kernel, :__defined_const_path?], [Kernel, :__defined_cvar?],
 [Kernel, :__defined_gvar?], [Kernel, :__defined_ivar?], [Kernel, :__defined_method?],
 [Kernel, :__defined_super?], [Kernel, :__defined_yield?],
 [Kernel, :`], [Kernel, :binding], [Kernel, :eval], [Kernel, :format],
 [Kernel, :global_variables], [Kernel, :local_variables], [Kernel, :proc],
 [Kernel, :respond_to_missing?], [Kernel, :sprintf],
 [Module, :const_added], [Module, :extended], [Module, :included], [Module, :method_added],
 [Module, :method_undefined], [Module, :module_function], [Module, :prepended],
 [Module, :private], [Module, :protected], [Module, :public], [Module, :remove_const],
 [Range, :initialize],
 [String, :initialize], [String, :initialize_copy]].each { |mod, name| show(mod, name) }

# Not in that list, and deliberately left alone: mruby-metaprog redefines Module#method_removed
# without MRB_MT_PRIVATE, and mruby-struct / mruby-random write an `initialize` ROM entry with no
# flag at all. A ROM table does not go through mrb_define_method_raw, so nothing makes them private.
show(Module, :method_removed)
show(Struct, :initialize)
show(Random, :initialize)
show(Struct, :initialize_copy)

# Module functions are a public singleton method and a private instance method of the same name.
ksc = Kernel.singleton_class.public_instance_methods(false)
puts "Kernel.sprintf: #{ksc.include?(:sprintf)}"
puts "Kernel.Rational: #{ksc.include?(:Rational)}"
puts "Kernel.global_variables: #{ksc.include?(:global_variables)}"
puts "sprintf works: #{sprintf('%d', 7)}"
puts "Kernel.sprintf works: #{Kernel.sprintf('%d', 8)}"

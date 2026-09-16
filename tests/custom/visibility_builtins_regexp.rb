# regexp-only: Regexp's share of the visibility list (leftovers-plan.md item 10).
# Expectation: the reference mruby's own output.
def show(mod, name)
  puts "#{mod}##{name}: #{mod.private_instance_methods(false).include?(name) ? 'private' : 'public'}"
end
show(Regexp, :initialize)
show(Regexp, :initialize_copy)
show(Regexp, :__check_initialized)
r = Regexp.new("a")
begin
  r.__check_initialized
rescue NoMethodError => e
  puts "explicit receiver: #{e.message}"
end
puts "match still works: #{r =~ 'bab'}"

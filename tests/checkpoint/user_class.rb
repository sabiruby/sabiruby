# checkpoint-test: suspend-after "built"
# User classes and modules: attributes, inheritance, an included module
# (and a permanent one, Comparable), class methods, class variables,
# constants, singleton methods on an object.
module Greeting
  def greet = "hello from #{name}"
end

class Animal
  include Comparable
  include Greeting
  attr_accessor :name, :legs
  @@count = 0
  KINGDOM = "animalia"

  def self.count = @@count

  def initialize(name, legs)
    @name = name
    @legs = legs
    @@count += 1
  end

  def <=>(other) = legs <=> other.legs
  def to_s = "#{self.class.name}(#{name}, #{legs})"
end

class Bird < Animal
  def initialize(name) = super(name, 2)
  def fly = "#{name} flies"
end

zoo = [Animal.new("cat", 4), Bird.new("crow"), Animal.new("snake", 0)]
odd = Animal.new("odd", 3)
def odd.special = "only me"
puts "built"
Checkpoint.suspend("u.ckpt")
zoo << Bird.new("owl")
puts zoo.sort.map(&:to_s).join(" ")
puts zoo[1].fly, zoo[0].greet, odd.special
puts "count=#{Animal.count} kingdom=#{Animal::KINGDOM} max=#{zoo.max}"
puts "#{Bird.ancestors.take(5).inspect} #{zoo[1].is_a?(Comparable)}"
zoo[0].legs = 5
puts zoo[0].legs, Bird.superclass, odd.singleton_methods.inspect

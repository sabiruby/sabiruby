//! The same macros reached through the VM crate: `sabiruby = { features = ["macros"] }` and
//! `use sabiruby::{RubyClass, ruby_methods};`, with no `sabiruby_macros` in sight.
//!
//! This is the shape a host outside this repository writes (the serde / serde_derive pattern),
//! and it is a test rather than a doc example because it has to *run*: what it checks is that
//! the generated code names `::sabiruby::…` absolutely, so it compiles the same whether the
//! macros arrive through the re-export or through a direct dependency on `sabiruby-macros`.
//! `player.rs` next to it is the direct-dependency half of that pair.
//!
//! One `use` brings two items called `RubyClass`: the derive macro (macro namespace) and the
//! trait it implements (type namespace). The trait is what a handle needs to be borrowed
//! through; `IntoRuby`, next to it, is the other half the derive writes.

use sabiruby::{IntoRuby, RubyClass, Value, Vm, ruby_methods};

#[derive(RubyClass)]
struct Counter {
    n: i64,
}

#[ruby_methods]
impl Counter {
    fn new(n: i64) -> Self {
        Counter { n }
    }
    fn step(&mut self, by: i64) -> i64 {
        self.n += by;
        self.n
    }
    fn n(&self) -> i64 {
        self.n
    }
    #[ruby(name = "zero?")]
    fn zero(&self) -> bool {
        self.n == 0
    }
}

fn run(vm: &mut Vm, src: &str) -> String {
    let bin = sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
        filename: "(test)".into(),
        debug_info: true,
        ..Default::default()
    })
    .expect("compile");
    vm.load_and_run(&bin).expect("run");
    String::from_utf8_lossy(&vm.take_output()).into_owned()
}

#[test]
fn the_reexported_macros_register_a_class_and_ruby_drives_it() {
    let mut vm = Vm::with_mrblib().expect("vm");
    let class = Counter::register(&mut vm).expect("Counter registers");

    let out = run(
        &mut vm,
        r#"
          c = Counter.new(0)
          p c.class
          p c.zero?
          p c.step(3)
          p c.step(4)
          p c.n
          p c.zero?
        "#,
    );
    assert_eq!(out, "Counter\ntrue\n3\n7\n7\nfalse\n");

    // `register` answers the class object itself (the trait's `register_class`), which is what
    // a host that wants to hang a constant or a subclass off it reads.
    assert_eq!(vm.class_name(class), "Counter");
}

#[test]
fn the_trait_that_came_with_the_same_use_is_the_one_the_handle_needs() {
    let mut vm = Vm::with_mrblib().expect("vm");
    Counter::register(&mut vm).expect("Counter registers");
    let obj = Counter { n: 41 }.into_ruby(&mut vm);

    // The value went into the store and Ruby holds a Data object naming it, not a copy.
    assert!(matches!(obj, Value::Obj(_)));
    // `RubyClass::borrow` — the trait item, in scope from the same single `use` above.
    assert_eq!(Counter::borrow(&mut vm, obj).expect("borrowed").n, 41);
}

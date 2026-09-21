//! Declarations written in Ruby, collected into a Rust table — and a Rust table read back
//! from a script.
//!
//! The vocabulary here is units of measure, which is an example and nothing else: the crate
//! knows no more about what is being declared than `serde` does.

use std::collections::BTreeMap;

use sabiruby::error::VmError;
use sabiruby::{Value, Vm};
use sabiruby_serde::declare::{expose, Declarations, OnDuplicate};
use serde::{Deserialize, Serialize};

fn vm() -> Vm {
    Vm::with_mrblib().expect("vm")
}

/// Compiles `src` under `filename` and runs it, as a host loading a data file would.
fn run(vm: &mut Vm, filename: &str, src: &str) -> Result<Value, VmError> {
    let bin = sabiruby_compiler::compile(src.as_bytes(), &sabiruby_compiler::Options {
        filename: filename.into(), debug_info: true, ..Default::default()
    }).expect("compile");
    vm.load_and_run(&bin)
}

/// What a script would see: the message, its class, and the first line of the backtrace.
fn raised(vm: &mut Vm, e: &VmError) -> String {
    let described = vm.describe_error(e);
    let VmError::Raise(exc) = e else { panic!("not a raise: {described}") };
    let exc = *exc;
    let mid = vm.intern("backtrace");
    let bt = vm.funcall(exc, mid, &[], Value::Nil).expect("backtrace");
    let first = vm.ary_vals(bt).and_then(|v| v.first().copied()).expect("a frame");
    let at = String::from_utf8_lossy(vm.str_bytes(first).expect("a String")).into_owned();
    format!("{described} at {at}")
}

/// The line of the first frame of a raised exception's backtrace — the number the host's own
/// complaints have to match.
fn raised_at(vm: &mut Vm, e: &VmError) -> u32 {
    let at = raised(vm, e);
    let (_, line) = at.rsplit_once(':').expect("a line");
    line.parse().expect("a number")
}

#[derive(Deserialize, Serialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct Unit { symbol: String, scale: f64 }

#[test]
fn declarations_arrive_in_order_as_a_rust_table() {
    let mut vm = vm();
    let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    run(&mut vm, "data.rb", concat!(
        "unit :metre, symbol: \"m\", scale: 1.0\n",
        "unit \"inch\", symbol: \"in\", scale: 0.0254\n",
        "unit :yard, scale: 0.9144, symbol: \"yd\"\n",
    )).expect("run");
    assert_eq!(units.len(&vm), 3);

    let table = units.take(&mut vm);
    // the order is the script's, and a name is a Symbol or a String as the script wrote it
    assert_eq!(table.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["metre", "inch", "yard"]);
    assert_eq!(table[1].1, Unit { symbol: "in".into(), scale: 0.0254 });
    // and the keywords are read by name, not by position
    assert_eq!(table[2].1, Unit { symbol: "yd".into(), scale: 0.9144 });
}

#[derive(Deserialize, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields, default)]
struct Flag { on: bool, note: Option<String> }

#[test]
fn a_declaration_without_keywords_is_the_same_as_an_empty_hash() {
    let mut vm = vm();
    let flags = Declarations::<Flag>::install(&mut vm).define(&mut vm, "flag");
    run(&mut vm, "data.rb", "flag :verbose\nflag :quiet, on: true\n").expect("run");
    let table = flags.take(&mut vm);
    assert_eq!(table[0], ("verbose".to_string(), Flag { on: false, note: None }));
    assert_eq!(table[1], ("quiet".to_string(), Flag { on: true, note: None }));
}

#[test]
fn a_missing_field_says_where_in_the_ruby_file_it_is() {
    let mut vm = vm();
    Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    let e = run(&mut vm, "data.rb", "# a data file\nunit :metre, symbol: \"m\"\n").expect_err("no scale");
    assert_eq!(raised(&mut vm, &e), "missing field `scale` (TypeError) at data.rb:2");
}

#[test]
fn a_field_of_the_wrong_type_says_where_it_is() {
    let mut vm = vm();
    Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    let e = run(&mut vm, "data.rb", "unit :metre, symbol: \"m\", scale: 1.0\nunit :inch, symbol: 5, scale: 1.0\n")
        .expect_err("symbol is not a String");
    assert_eq!(raised(&mut vm, &e), "cannot deserialize Integer as String (TypeError) at data.rb:2");
}

#[test]
fn an_unknown_field_says_where_it_is() {
    let mut vm = vm();
    Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    let e = run(&mut vm, "data.rb", "unit :metre,\n     symbol: \"m\",\n     scale: 1.0,\n     colour: \"red\"\n")
        .expect_err("deny_unknown_fields");
    // a call spread over four lines is located at its last one, which is the line the SEND
    // instruction carries — the offending field is on that line here, which is the useful half
    assert_eq!(raised(&mut vm, &e),
        "unknown field `colour`, expected `symbol` or `scale` (TypeError) at data.rb:4");
}

#[test]
fn a_name_declared_twice_is_an_argument_error_at_the_second_one() {
    let mut vm = vm();
    let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    let e = run(&mut vm, "data.rb", concat!(
        "unit :metre, symbol: \"m\", scale: 1.0\n",
        "unit :inch, symbol: \"in\", scale: 0.0254\n",
        "unit :metre, symbol: \"M\", scale: 1.0\n",
    )).expect_err("declared twice");
    assert_eq!(raised(&mut vm, &e), "unit \"metre\" is already declared (ArgumentError) at data.rb:3");
    // the first two are still there, and the second `metre` did not overwrite the first
    let table = units.take(&mut vm);
    assert_eq!(table.len(), 2);
    assert_eq!(table[0].1.symbol, "m");
}

#[test]
fn the_door_that_allows_overwriting_has_its_own_name_and_keeps_the_order() {
    let mut vm = vm();
    let units = Declarations::<Unit>::install(&mut vm)
        .define(&mut vm, "unit")
        .define_replacing(&mut vm, "unit!");
    run(&mut vm, "data.rb", concat!(
        "unit :metre, symbol: \"m\", scale: 1.0\n",
        "unit :inch, symbol: \"in\", scale: 0.0254\n",
        "unit! :metre, symbol: \"M\", scale: 1.0\n",
        "unit! :yard, symbol: \"yd\", scale: 0.9144\n",
    )).expect("run");
    let table = units.take(&mut vm);
    // `metre` was replaced where it stood; a name the table has not got is simply added
    assert_eq!(table.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["metre", "inch", "yard"]);
    assert_eq!(table[0].1.symbol, "M");
}

#[test]
fn a_name_that_is_not_a_symbol_or_a_string_is_a_type_error() {
    let mut vm = vm();
    let object = vm.core.object;
    Declarations::<Unit>::install(&mut vm).define_on(&mut vm, object, "unit", OnDuplicate::Raise);
    let e = run(&mut vm, "data.rb", "unit 7, symbol: \"m\", scale: 1.0\n").expect_err("not a name");
    assert_eq!(raised(&mut vm, &e),
        "wrong argument type Integer (expected Symbol or String) (TypeError) at data.rb:1");
}

#[test]
fn taking_the_table_twice_answers_with_nothing_the_second_time() {
    let mut vm = vm();
    let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    run(&mut vm, "data.rb", "unit :metre, symbol: \"m\", scale: 1.0\n").expect("run");
    assert_eq!(units.take(&mut vm).len(), 1);
    assert_eq!(units.take(&mut vm).len(), 0);
    assert_eq!(units.len(&vm), 0);
    assert!(units.is_empty(&vm));
}

#[test]
fn declaring_after_the_table_was_taken_raises_rather_than_being_dropped() {
    let mut vm = vm();
    let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    run(&mut vm, "data.rb", "unit :metre, symbol: \"m\", scale: 1.0\n").expect("run");
    units.take(&mut vm);
    let e = run(&mut vm, "more.rb", "unit :inch, symbol: \"in\", scale: 0.0254\n").expect_err("gone");
    assert_eq!(raised(&mut vm, &e),
        "unit: the host has already taken this table out of the VM (RuntimeError) at more.rb:1");
}

// ------------------------------------------------------------------ the line a declaration was on

/// A file whose second declaration is spread over two lines, `$1` standing in for what `scale`
/// is given — the one thing that decides whether the file runs or raises.
fn over_two_lines(scale: &str) -> String {
    format!(concat!(
        "# a data file\n",
        "unit :metre, symbol: \"m\", scale: 1.0\n",
        "unit :inch,\n",
        "     symbol: \"in\", scale: {}\n",
        "unit :yard, symbol: \"yd\", scale: 0.9144\n",
    ), scale)
}

#[test]
fn a_declaration_spread_over_two_lines_remembers_the_line_an_error_in_it_would_name() {
    // what the VM says when the declaration is wrong: the line the SEND carries, which is the
    // line the declaration *ends* on, not the one it starts on
    let mut vm = vm();
    Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    let e = run(&mut vm, "data.rb", &over_two_lines("\"not a number\"")).expect_err("not a Float");
    let complained_at = raised_at(&mut vm, &e);
    assert_eq!(complained_at, 4);

    // and what the table remembers when the same declaration is right: the same number, from
    // the same place. A host checking one declaration against another can now say where.
    let mut vm = Vm::with_mrblib().expect("vm");
    let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    run(&mut vm, "data.rb", &over_two_lines("0.0254")).expect("run");
    let table = units.take_with_lines(&mut vm);
    assert_eq!(table.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["metre", "inch", "yard"]);
    assert_eq!(table.iter().map(|d| d.line).collect::<Vec<_>>(),
        [Some(2), Some(complained_at), Some(5)]);
    assert_eq!(table[1].value, Unit { symbol: "in".into(), scale: 0.0254 });
}

#[test]
fn take_is_take_with_lines_without_the_lines() {
    let mut vm = vm();
    let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    run(&mut vm, "data.rb", &over_two_lines("0.0254")).expect("run");
    // the older entry point answers exactly as it did before there were lines to keep
    let table: Vec<(String, Unit)> = units.take(&mut vm);
    assert_eq!(table.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["metre", "inch", "yard"]);
    assert_eq!(table[1].1, Unit { symbol: "in".into(), scale: 0.0254 });
    // and it empties the table the same way
    assert!(units.take_with_lines(&mut vm).is_empty());
}

#[test]
fn an_amended_declaration_keeps_its_place_but_takes_the_later_line() {
    let mut vm = vm();
    let units = Declarations::<Unit>::install(&mut vm)
        .define(&mut vm, "unit")
        .define_replacing(&mut vm, "unit!");
    run(&mut vm, "data.rb", concat!(
        "unit :metre, symbol: \"m\", scale: 1.0\n",
        "unit :inch, symbol: \"in\", scale: 0.0254\n",
        "unit! :metre, symbol: \"M\", scale: 1.0\n",
    )).expect("run");
    let table = units.take_with_lines(&mut vm);
    assert_eq!(table.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["metre", "inch"]);
    // `metre` stayed where it was declared, but the value and the line are the amendment's:
    // line 3 is where what the host now holds was written
    assert_eq!(table[0].value.symbol, "M");
    assert_eq!(table.iter().map(|d| d.line).collect::<Vec<_>>(), [Some(3), Some(2)]);
}

#[test]
fn a_declaration_from_bytecode_without_debug_information_has_no_line() {
    let mut vm = vm();
    let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    let bin = sabiruby_compiler::compile(b"unit :metre, symbol: \"m\", scale: 1.0\n",
        &sabiruby_compiler::Options { filename: "data.rb".into(), debug_info: false, ..Default::default() })
        .expect("compile");
    vm.load_and_run(&bin).expect("run");
    let table = units.take_with_lines(&mut vm);
    // there is no line to give, and `Exception#backtrace` is empty in such a file too: an
    // absent line is `None` rather than a 0 a host might print
    assert_eq!(table[0].line, None);
    assert_eq!(table[0].name, "metre");
}

/// The live objects a VM is left with after the declarations have been taken and the collector
/// has run — with the script, and with nothing declared at all.
fn live_after(script: &str) -> usize {
    let mut vm = vm();
    let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    run(&mut vm, "data.rb", script).expect("run");
    units.take(&mut vm);
    vm.gc_collect();
    vm.heap.live_count()
}

#[test]
fn the_declarations_hold_no_ruby_object_and_leave_nothing_behind() {
    let mut vm = vm();
    let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    // a collection in the middle of the data file must not disturb the table
    run(&mut vm, "data.rb", concat!(
        "unit :metre, symbol: \"m\", scale: 1.0\n",
        "GC.start\n",
        "unit :inch, symbol: \"in\", scale: 0.0254\n",
    )).expect("run");
    vm.gc_collect();
    let table = units.take(&mut vm);
    assert_eq!(table[0].1.symbol, "m");
    assert_eq!(table[1].1.symbol, "in");

    // and nothing of a declaration outlives the taking: the names and the values are Rust's,
    // so the VM is left where a VM that was given no declarations at all is left
    let declared = concat!(
        "unit :metre, symbol: \"m\", scale: 1.0\n",
        "unit :inch, symbol: \"in\", scale: 0.0254\n",
        "unit :yard, symbol: \"yd\", scale: 0.9144\n",
    );
    assert_eq!(live_after(declared), live_after(""));
}

#[test]
fn a_host_table_is_read_back_from_ruby_with_symbol_keys() {
    let mut vm = vm();
    let table = vec![
        ("metre".to_string(), Unit { symbol: "m".into(), scale: 1.0 }),
        ("inch".to_string(), Unit { symbol: "in".into(), scale: 0.0254 }),
    ];
    let units = expose(&mut vm, "unit_of", table);
    run(&mut vm, "control.rb", concat!(
        "$one = unit_of(:metre)\n",
        "$two = unit_of(\"inch\")[:scale]\n",
        "$none = unit_of(:furlong)\n",
        "$keys = unit_of(:metre).keys.inspect\n",
    )).expect("run");
    let one = vm.global_get("$one");
    let key = Value::Sym(vm.intern("scale"));
    let scale = vm.hash_get(one, key).expect("scale");
    assert_eq!(scale, Value::Float(1.0));
    assert_eq!(vm.global_get("$two"), Value::Float(0.0254));
    assert_eq!(vm.global_get("$none"), Value::Nil);
    let keys = vm.global_get("$keys");
    assert_eq!(String::from_utf8_lossy(vm.str_bytes(keys).expect("a String")), "[:symbol, :scale]");
    assert_eq!(units.len(&vm), 2);

    // and the same table can be taken back out
    let back = units.take(&mut vm);
    assert_eq!(back.len(), 2);
    let e = run(&mut vm, "control.rb", "unit_of(:metre)\n").expect_err("gone");
    assert_eq!(raised(&mut vm, &e),
        "unit_of: the host has already taken this table out of the VM (RuntimeError) at control.rb:1");
}

/// A published entry with a map in it: the shape Factory's recipes have — the ingredients are
/// names of other declared things, kept in Rust as a map's keys.
#[derive(Serialize)]
struct Recipe {
    ingredients: BTreeMap<String, u32>,
    time: f64,
}

#[test]
fn the_names_inside_a_published_entry_read_back_as_the_symbols_they_were_written_as() {
    let mut vm = vm();
    let mut ingredients = BTreeMap::new();
    ingredients.insert("iron_ore".to_string(), 1);
    expose(&mut vm, "recipe_of", [("iron_plate".to_string(), Recipe { ingredients, time: 2.0 })]);
    run(&mut vm, "control.rb", concat!(
        "$whole = recipe_of(:iron_plate).inspect\n",
        "$n = recipe_of(:iron_plate)[:ingredients][:iron_ore]\n",
    )).expect("run");
    let whole = vm.global_get("$whole");
    // the field names and the names *inside* the entry are both Symbols: a data file that
    // wrote `in: { iron_ore: 1 }` reads it back the same way round
    assert_eq!(String::from_utf8_lossy(vm.str_bytes(whole).expect("a String")),
        "{ingredients: {iron_ore: 1}, time: 2.0}");
    assert_eq!(vm.global_get("$n"), Value::Int(1));
}

#[test]
fn collecting_and_exposing_the_same_type_are_two_tables_in_one_vm() {
    let mut vm = vm();
    let units = Declarations::<Unit>::install(&mut vm).define(&mut vm, "unit");
    run(&mut vm, "data.rb", "unit :metre, symbol: \"m\", scale: 1.0\n").expect("run");
    let collected = units.take(&mut vm);
    // what the host publishes is what it decided to publish, after whatever checking it does
    let exposed = expose(&mut vm, "unit_of", collected);
    run(&mut vm, "control.rb", "$s = unit_of(:metre)[:symbol]\n").expect("run");
    let s = vm.global_get("$s");
    assert_eq!(String::from_utf8_lossy(vm.str_bytes(s).expect("a String")), "m");
    assert_eq!(exposed.len(&vm), 1);
}

#[test]
fn two_vms_keep_their_own_tables() {
    let mut a = vm();
    let mut b = vm();
    let ta = Declarations::<Unit>::install(&mut a).define(&mut a, "unit");
    let tb = Declarations::<Unit>::install(&mut b).define(&mut b, "unit");
    run(&mut a, "a.rb", "unit :metre, symbol: \"m\", scale: 1.0\n").expect("run a");
    run(&mut b, "b.rb", "unit :inch, symbol: \"in\", scale: 0.0254\nunit :yard, symbol: \"yd\", scale: 0.9144\n").expect("run b");
    assert_eq!(ta.take(&mut a).len(), 1);
    assert_eq!(tb.take(&mut b).len(), 2);
}

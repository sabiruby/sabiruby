//! Bytecode the compiler never writes, as mruby 4.1.0-rc2 refuses it. Both checks came with rc2
//! as ordinary fixes (malformed bytecode is not a security boundary, the reference's
//! SECURITY.md), and the reference tests them only in its bintest (mruby-bin-mruby), so the
//! images here are forged the way that bintest forges them, and the expectations are what the
//! reference `mruby -b` answers for the same bytes (`kishima/mruby:4.1.0-rc2`).

use sabiruby::Vm;

/// The offset of the irep record after the top-level one (the first child), as the bintest
/// finds it: the IREP section's header is 12 bytes (ident, size, version), and a record starts
/// with its own size.
fn second_record(image: &[u8]) -> usize {
    let irep = image.windows(4).position(|w| w == b"IREP").expect("IREP section");
    let first = irep + 12;
    first + u32::from_be_bytes(image[first..first + 4].try_into().unwrap()) as usize
}

#[test]
fn a_record_with_more_locals_than_registers_is_refused() {
    // load.c, 7ed51715e: a frame has `nregs` registers and its locals are the first of them.
    // Before, SabiRuby loaded the record and `OP_ENTER` panicked clearing past the frame.
    let image = sabiruby_compiler::compile(b"def m(a); a; end\nm(1)\n", &sabiruby_compiler::Options {
        filename: "(test)".into(), remove_lv: true, ..Default::default()
    }).expect("compile");
    let at = second_record(&image);
    let nlocals = u16::from_be_bytes([image[at + 4], image[at + 5]]);
    let nregs = u16::from_be_bytes([image[at + 6], image[at + 7]]);
    assert!(nlocals <= nregs, "the compiler wrote nlocals={nlocals} nregs={nregs}");

    for forged_nlocals in [nregs + 1, 0xffff] {
        let mut forged = image.clone();
        forged[at + 4..at + 6].copy_from_slice(&forged_nlocals.to_be_bytes());
        let mut vm = Vm::new();
        match vm.load(&forged) {
            Err(sabiruby::VmError::Rite(msg)) => assert!(msg.contains("locals"), "{msg}"),
            other => panic!("nlocals={forged_nlocals} nregs={nregs}: expected a refusal, got {other:?}"),
        }
    }
    // the image it was forged from still runs
    let mut vm = Vm::new();
    vm.load_and_run(&image).expect("the compiler's own image runs");
}

/// A one-record image (nlocals 1, nregs 4) of `iseq` and `pool`, laid out as the bintest's
/// `image` lambda lays it out.
fn image(iseq: &[u8], pool: &[Vec<u8>]) -> Vec<u8> {
    let mut body = Vec::new();
    for n in [1u16, 4, 0, 0] { body.extend_from_slice(&n.to_be_bytes()); }
    body.extend_from_slice(&(iseq.len() as u32).to_be_bytes());
    body.extend_from_slice(iseq);
    body.extend_from_slice(&(pool.len() as u16).to_be_bytes());
    for p in pool { body.extend_from_slice(p); }
    body.extend_from_slice(&0u16.to_be_bytes());
    let mut rec = ((4 + body.len()) as u32).to_be_bytes().to_vec();
    rec.extend_from_slice(&body);
    let mut irep = b"IREP".to_vec();
    irep.extend_from_slice(&((8 + 4 + rec.len()) as u32).to_be_bytes());
    irep.extend_from_slice(b"0400");
    irep.extend_from_slice(&rec);
    let fin = [b"END\0".as_slice(), &8u32.to_be_bytes()].concat();
    let total = 8 + 4 + 8 + irep.len() + fin.len();
    let mut out = b"RITE0400".to_vec();
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(b"MRB\x000000");
    out.extend_from_slice(&irep);
    out.extend_from_slice(&fin);
    out
}

#[test]
fn op_call_on_a_receiver_that_is_not_a_proc_names_what_it_found() {
    // vm.c, 78a055595. SabiRuby already raised, without the type in the message; the reference
    // says `wrong type %T (expected Proc)`. The opcode numbers are their place in ops.h.
    use sabiruby::opcode::Op;
    let (loadi_0, string, call, stop) = (Op::Loadi0 as u8, Op::String as u8, Op::Call as u8, Op::Stop as u8);
    assert_eq!([loadi_0, string, call, stop], [6, 92, 53, 118]);
    let s = b"AAAAAAAA";
    let mut str_entry = vec![0u8];
    str_entry.extend_from_slice(&(s.len() as u16).to_be_bytes());
    str_entry.extend_from_slice(s);
    str_entry.push(0);
    let cases = [
        ("Integer", image(&[loadi_0, 0, call, stop], &[])),
        ("String", image(&[string, 0, 0, call, stop], &[str_entry])),
    ];
    for (ty, bytes) in cases {
        let mut vm = Vm::new();
        let e = vm.load_and_run(&bytes).expect_err("OP_CALL on a non-Proc raises");
        assert_eq!(vm.describe_error(&e), format!("wrong type {ty} (expected Proc) (TypeError)"));
    }
    // a Proc still answers where one really was put
    let bin = sabiruby_compiler::compile(b"Proc.new { 41 + 1 }.call", &Default::default()).expect("compile");
    let mut vm = Vm::new();
    assert_eq!(vm.load_and_run(&bin).expect("run"), sabiruby::Value::Int(42));
}

//! The feature `checkpoint`: every image in `tests/checkpoint/` (written by C mruby with its
//! checkpoint gem from the `.rb` beside it, `docs/plans/checkpoint-plan.md`) is resumed on a
//! fresh VM, and what the program prints from there to its end must be the `.out` beside it
//! (what the writer's reference run printed after that point), or the restore must be refused
//! with a message containing the `.refused` beside it. `NAME.K` is the K-th image along the
//! program's chain of suspends; `NAME.1-32` the first one written by a 32-bit build.
#![cfg(feature = "checkpoint")]

use std::path::{Path, PathBuf};

fn dir() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/checkpoint") }

fn fresh_vm() -> sabiruby::Vm {
    let mut vm = sabiruby::Vm::new();
    vm.set_gc_stress(std::env::var("SABIRUBY_GC_STRESS").map(|v| !v.is_empty() && v != "0").unwrap_or(false));
    vm.load_mrblib().expect("mrblib loads");
    // the writer's driver gives the program an empty ARGV, as the `sabiruby` command does
    let argv = vm.ary_new(vec![]);
    let n = vm.intern("ARGV");
    vm.heap.class_mut(vm.core.object).consts.insert(n, sabiruby::value::Slot::from(argv));
    vm
}

/// Resumes `stem.ckpt`: Ok(output) or Err(message).
fn resume(vm: &mut sabiruby::Vm, stem: &str) -> Result<String, String> {
    let img = std::fs::read(dir().join(format!("{stem}.ckpt"))).expect("image");
    let r = vm.checkpoint_resume(&img);
    let out = String::from_utf8_lossy(&vm.take_output()).into_owned();
    match r {
        Ok(_) => Ok(out),
        Err(e) => Err(format!("{out}{}", vm.describe_error(&e))),
    }
}

fn stems() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir()).expect("tests/checkpoint").filter_map(|e| {
        let p = e.ok()?.path();
        (p.extension()? == "ckpt").then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
    }).collect();
    v.sort();
    v
}

#[test]
fn images_resume() {
    let stems = stems();
    assert!(stems.len() >= 20, "the images are in tests/checkpoint");
    let mut failed = Vec::new();
    for stem in &stems {
        let mut vm = fresh_vm();
        let got = resume(&mut vm, stem);
        let out = dir().join(format!("{stem}.out"));
        let refused = dir().join(format!("{stem}.refused"));
        let ok = if out.exists() {
            let want = std::fs::read_to_string(&out).unwrap();
            match &got { Ok(o) => o == &want, Err(_) => false }
        } else {
            let why = std::fs::read_to_string(&refused).expect(".out or .refused");
            match &got { Err(e) => e.contains(why.trim()), Ok(_) => false }
        };
        if !ok { failed.push(format!("{stem}: {got:?}")); }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// A refused image leaves the VM idle: it runs a program afterwards as a fresh one does.
#[test]
fn refused_leaves_vm_idle() {
    let mut vm = fresh_vm();
    assert!(resume(&mut vm, "sabiruby_refuse.1").is_err());
    let mut ok = fresh_vm();
    let a = resume(&mut vm, "counter.1").expect("resumes after a refusal");
    let b = resume(&mut ok, "counter.1").unwrap();
    assert_eq!(a, b);
}

/// Every cut of an image is refused with an error, never a panic. (A cut image fails while it
/// is read, before anything is built, so one VM serves for all of them.)
#[test]
fn truncated_images_are_refused() {
    let img = std::fs::read(dir().join("user_class.1.ckpt")).unwrap();
    let mut vm = fresh_vm();
    for n in 0..img.len() {
        let r = vm.checkpoint_resume(&img[..n]);
        assert!(r.is_err(), "a cut at {n} of {} bytes was accepted", img.len());
    }
}

/// An image whose carried code was changed is refused by the content hash.
#[test]
fn changed_code_is_refused() {
    let mut img = std::fs::read(dir().join("counter.1.ckpt")).unwrap();
    // the first irep's instructions start after the symbols and the irep's fixed fields; flip
    // a byte well inside the record, which is code
    let (v, _) = sabiruby::Vm::checkpoint_header(&img).unwrap();
    assert_eq!(v, 0);
    let at = img.windows(4).position(|w| w == b"done").expect("a pool string") - 8;
    img[at] ^= 0xff;
    let mut vm = fresh_vm();
    let e = match vm.checkpoint_resume(&img) { Err(e) => vm.describe_error(&e), Ok(_) => panic!("accepted") };
    assert!(e.contains("restore failed"), "{e}");
}

/// The 64-bit and the 32-bit writer's images differ in the byte that records the writer's
/// pointer width only.
#[test]
fn image_is_the_same_from_32_bit() {
    for s in ["counter", "user_class", "hash"] {
        let a = std::fs::read(dir().join(format!("{s}.1.ckpt"))).unwrap();
        let b = std::fs::read(dir().join(format!("{s}.1-32.ckpt"))).unwrap();
        assert_eq!(a.len(), b.len());
        let diffs: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
        assert_eq!(diffs.len(), 1, "{s}");
    }
}

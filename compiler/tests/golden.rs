//! Byte-for-byte comparison with the reference `mrbc` (mruby 4.1.0-rc2, Docker image
//! `kishima/mruby:4.1.0-rc2`): every `.rb` of the repository that has a `.mrb` made by it
//! must compile to exactly that `.mrb`. The file name matters when `-g` puts it into the
//! DBG section, so each set uses the name the generating script passed to `mrbc`.

use std::path::{Path, PathBuf};

use sabiruby_compiler::{compile, Kind, Options};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Compiles every `src_dir/*.rb` whose `.mrb` exists in `mrb_dir`; `name` gives the file
/// name `mrbc` saw. Returns the number compared; panics listing every mismatch.
fn golden(src_dir: &str, mrb_dir: &str, debug_info: bool, name: impl Fn(&str) -> String) -> usize {
    let mut rbs: Vec<PathBuf> = std::fs::read_dir(repo().join(src_dir)).unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rb"))
        .collect();
    rbs.sort();
    let (mut n, mut bad) = (0, vec![]);
    for rb in rbs {
        let stem = rb.file_stem().unwrap().to_string_lossy().into_owned();
        let mrb = repo().join(mrb_dir).join(format!("{stem}.mrb"));
        let Ok(expected) = std::fs::read(&mrb) else { continue };
        let src = std::fs::read(&rb).unwrap();
        let opts = Options { filename: name(&stem), debug_info, ..Default::default() };
        match compile(&src, &opts) {
            Ok(bin) if bin == expected => {}
            Ok(bin) => {
                let at = bin.iter().zip(&expected).position(|(a, b)| a != b).unwrap_or(bin.len().min(expected.len()));
                bad.push(format!("{stem}: {} bytes vs {} from mrbc, first difference at {at}", bin.len(), expected.len()));
            }
            Err(e) => bad.push(format!("{stem}: {e}")),
        }
        n += 1;
    }
    assert!(bad.is_empty(), "{} of {n} differ from mrbc:\n{}", bad.len(), bad.join("\n"));
    n
}

#[test]
fn fixtures_match_mrbc() {
    // tools/fixtures.sh: mrbc -o /w/NAME.mrb /w/NAME.rb
    let n = golden("tests/fixtures", "tests/fixtures", false, |s| format!("/w/{s}.rb"));
    assert!(n >= 17, "only {n} fixtures");
}

#[test]
fn mrbtest_matches_mrbc() {
    // tools/mrbtest.sh: mrbc -g -o /w/NAME.mrb /w/src/NAME.rb (assert.rb included)
    let n = golden("tests/mrbtest/src", "tests/mrbtest", true, |s| format!("/w/src/{s}.rb"));
    assert!(n >= 60, "only {n} test files");
}

#[test]
fn bench_matches_mrbc() {
    // tools/bench.sh: cd /w && mrbc -o NAME.mrb src/NAME.rb
    golden("bench/src", "bench", false, |s| format!("src/{s}.rb"));
}

#[test]
fn errors_are_diagnostics() {
    for src in [&b"def"[..], b"x = 1 +"] {
        let e = compile(src, &Options::default()).unwrap_err();
        assert_eq!(e.diagnostics[0].kind, Kind::ParserError, "{:?}", e);
        assert_eq!(e.diagnostics[0].line, 1);
        assert_eq!(e.diagnostics[0].filename, "-e");
    }
    let e = compile(b"1 +", &Options { filename: "t.rb".into(), ..Default::default() }).unwrap_err();
    assert!(e.to_string().starts_with("t.rb:1:"), "{e}");
}

#[test]
fn warnings_do_not_fail() {
    // an unused literal in void context is a parser warning, not an error
    let r = compile(b"1\nnil\np 2", &Options::default());
    assert!(r.is_ok());
}

#[test]
fn parallel_compiles() {
    let hs: Vec<_> = (0..8).map(|i| std::thread::spawn(move || compile(format!("p {i}").as_bytes(), &Options::default()).unwrap())).collect();
    for h in hs { assert_eq!(&h.join().unwrap()[..4], b"RITE"); }
}

#[cfg(feature = "ast")]
#[test]
fn ast_is_prisms_pretty_print() {
    let t = sabiruby_compiler::ast(b"def add(a, b) = a + b\nx = add(1, 2)\n", "t.rb").unwrap();
    assert!(t.starts_with("@ ProgramNode (location: (1,0)-(2,13))\n+-- locals: [:x]\n+-- statements:\n"), "{t}");
    assert!(t.contains("+-- @ DefNode (location: (1,0)-(1,21))") && t.contains("+-- name: :add"), "{t}");
    // a syntax error still gives a tree; the error itself comes from compile()
    assert!(sabiruby_compiler::ast(b"x = 1 +", "t.rb").unwrap().starts_with("@ ProgramNode"));
    assert!(sabiruby_compiler::ast(b"__FILE__", "t.rb").unwrap().contains("filepath: \"t.rb\""));
}

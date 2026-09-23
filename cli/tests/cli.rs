//! The command line, checked against the behaviour of the reference `mruby` (4.1.0-rc2):
//! `sabiruby [switches] [programfile] [arguments]`, plus the extra subcommands.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}
fn sabiruby(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sabiruby")).args(args).current_dir(repo()).output().expect("run sabiruby")
}
fn out(o: &Output) -> String { String::from_utf8_lossy(&o.stdout).into_owned() }
fn err(o: &Output) -> String { String::from_utf8_lossy(&o.stderr).into_owned() }
fn code(o: &Output) -> i32 { o.status.code().unwrap_or(-1) }

/// A temporary file with `content`, removed when the test ends.
struct Temp(PathBuf);
impl Temp {
    fn new(name: &str, content: &[u8]) -> Temp {
        let p = std::env::temp_dir().join(format!("sabiruby-cli-{}-{name}", std::process::id()));
        std::fs::write(&p, content).expect("write");
        Temp(p)
    }
    fn path(&self) -> &str { self.0.to_str().unwrap() }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
}

#[test]
fn runs_a_ruby_file_without_a_subcommand() {
    let f = Temp::new("argv.rb", b"p ARGV\np $DEBUG\n");
    let o = sabiruby(&[f.path(), "x", "-y"]);
    assert_eq!(out(&o), "[\"x\", \"-y\"]\nfalse\n", "{}", err(&o));
    assert_eq!(code(&o), 0);
    // -d sets $DEBUG, as mruby
    assert_eq!(out(&sabiruby(&["-d", f.path()])), "[]\ntrue\n");
}

#[test]
fn runs_bytecode_without_a_subcommand() {
    let o = sabiruby(&["tests/fixtures/hello.mrb"]);
    assert_eq!(out(&o), std::fs::read_to_string(repo().join("tests/fixtures/hello.out")).unwrap());
}

#[test]
fn run_subcommand_still_works() {
    // tools/*.sh use it
    let o = sabiruby(&["run", "--stats", "tests/fixtures/hello.mrb"]);
    assert_eq!(out(&o), "hello\n3\n");
    assert!(err(&o).contains("instructions: "), "{}", err(&o));
}

#[test]
fn dash_e_and_its_arguments() {
    assert_eq!(out(&sabiruby(&["-e", "p ARGV", "foo", "bar"])), "[\"foo\", \"bar\"]\n");
    assert_eq!(out(&sabiruby(&["-e", "a = 1", "-e", "p a"])), "1\n"); // joined, as mruby
}

#[test]
fn reads_the_program_from_stdin() {
    let mut c = Command::new(env!("CARGO_BIN_EXE_sabiruby")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("spawn");
    c.stdin.take().unwrap().write_all(b"puts 1 + 1\n").unwrap();
    let o = c.wait_with_output().unwrap();
    assert_eq!(out(&o), "2\n");
}

#[test]
fn check_syntax_only() {
    let ok = Temp::new("ok.rb", b"p 1\n");
    let o = sabiruby(&["-c", ok.path()]);
    assert_eq!(out(&o), "Syntax OK\n");
    assert_eq!(code(&o), 0);

    let bad = Temp::new("bad.rb", b"x = 1 +\n");
    let o = sabiruby(&["-c", bad.path()]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains(":1:8: syntax error"), "{}", err(&o));
    assert_eq!(out(&o), "");
}

#[test]
fn errors_look_like_mrubys() {
    let o = sabiruby(&["nope.rb"]);
    assert_eq!(err(&o), "sabiruby: Cannot open program file: nope.rb\n");
    assert_eq!(code(&o), 1);

    let rb = Temp::new("plain.rb", b"p 1\n");
    let o = sabiruby(&["-b", rb.path()]); // -b: bytecode only
    assert!(err(&o).starts_with("sabiruby: Cannot load RiteBinary: "), "{}", err(&o));
    assert_eq!(code(&o), 1);

    let o = sabiruby(&["-e", "raise 'boom'"]);
    assert_eq!(err(&o), "boom (RuntimeError)\n");
    assert_eq!(code(&o), 1);
}

#[test]
fn version_copyright_and_verbose() {
    let v = out(&sabiruby(&["--version"]));
    assert!(v.starts_with(&format!("sabiruby {}", env!("CARGO_PKG_VERSION"))) && v.contains("compiler: mruby 4.1.0-rc2"), "{v}");
    assert!(out(&sabiruby(&["--copyright"])).contains("mruby developers"));
    // -v prints the version, then the listing, then runs (as mruby -v)
    let o = out(&sabiruby(&["-v", "-e", "p 1"]));
    assert!(o.starts_with("sabiruby "), "{o}");
    assert!(o.contains("irep 0 nregs="), "{o}");
    assert!(o.trim_end().ends_with("1"), "{o}");
}

#[test]
fn minus_r_loads_a_library_first() {
    // the reference `mruby` loads each -r file before the program; here that is `Kernel#load`
    let lib = Temp::new("rlib.rb", b"p :lib\nLIB = 1\n");
    let o = sabiruby(&["-r", lib.path(), "-e", "p LIB"]);
    assert_eq!(out(&o), ":lib\n1\n", "{}", err(&o));
    assert_eq!(code(&o), 0);
    // a library that is not there is a LoadError before the program runs
    let o = sabiruby(&["-r", "/no/such/lib.rb", "-e", "p 1"]);
    assert!(err(&o).contains("cannot load such file -- /no/such/lib.rb"), "{}", err(&o));
    assert_eq!(code(&o), 1);
}

#[test]
fn require_searches_the_programs_directory() {
    // `$LOAD_PATH` is the program's own directory and the working directory
    let dir = std::env::temp_dir().join(format!("sabiruby-cli-{}-req", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("helper.rb"), b"HELPER = :here\n").expect("write");
    std::fs::write(dir.join("prog.rb"), b"p require('helper')\np HELPER\np require('helper')\n").expect("write");
    let o = sabiruby(&[dir.join("prog.rb").to_str().unwrap()]);
    assert_eq!(out(&o), "true\n:here\nfalse\n", "{}", err(&o));
    assert_eq!(code(&o), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compile_matches_the_reference_mrbc() {
    // tests/fixtures/hello.mrb was made by the reference mrbc without -g, so the file name
    // does not enter the binary: the bytes must be identical
    let dir = std::env::temp_dir().join(format!("sabiruby-cli-{}-out.mrb", std::process::id()));
    let o = sabiruby(&["compile", "tests/fixtures/hello.rb", "-o", dir.to_str().unwrap()]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(std::fs::read(&dir).unwrap(), std::fs::read(repo().join("tests/fixtures/hello.mrb")).unwrap());
    let _ = std::fs::remove_file(&dir);

    assert_eq!(out(&sabiruby(&["compile", "-c", "tests/fixtures/hello.rb"])), "Syntax OK\n");
}

#[test]
fn dump_reads_source_and_bytecode() {
    // from source the CLI compiles with -g, so the listing has the line column; the committed
    // .mrb was made without it. The instructions themselves must be the same.
    let a = out(&sabiruby(&["dump", "tests/fixtures/hello.rb"]));
    let b = out(&sabiruby(&["dump", "tests/fixtures/hello.mrb"]));
    let strip = |s: &str| s.lines().map(|l| l.get(6..).unwrap_or(l).to_string()).collect::<Vec<_>>().join("\n");
    assert_eq!(strip(&a), strip(&b));
    assert!(a.starts_with("irep 0 nregs="), "{a}");
    assert!(a.contains("    1 000 STRING"), "no line numbers from -g:\n{a}");
    assert!(b.contains("      000 STRING"), "unexpected line numbers without -g:\n{b}");
}

#[test]
fn help_lists_the_switches_and_the_subcommands() {
    let h = out(&sabiruby(&["--help"]));
    for s in ["-b", "-c", "-d", "-e", "-r", "-v", "--verbose", "--version", "--copyright", "--stats", "compile", "dump", "mrbtest", "run"] {
        assert!(h.contains(s), "--help does not mention {s}:\n{h}");
    }
    assert_eq!(code(&sabiruby(&["--help"])), 0);
}

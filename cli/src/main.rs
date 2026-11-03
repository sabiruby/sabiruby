//! `sabiruby` command line (crate `sabiruby-cli`): the switches of the reference `mruby`
//! command over the SabiRuby VM, plus `compile` (as `mrbc`), `dump` and `mrbtest`.
//! Ruby source is compiled by the reference compiler (crate `sabiruby-compiler`).

use std::io::{Read, Write};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod edge_spec;

/// `SABIRUBY_GC_STRESS=1`: collect at every instruction boundary after an allocation.
fn gc_stress() -> bool {
    std::env::var("SABIRUBY_GC_STRESS").map(|v| !v.is_empty() && v != "0").unwrap_or(false)
}

/// Nanoseconds since the first call (the library has no clock of its own).
/// Seconds and nanoseconds since the Unix epoch (`Time.now`).
fn wall_clock() -> (i64, i64) {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos() as i64),
        Err(_) => (0, 0),
    }
}

/// Where the program's output goes as it is written (`Vm::output_sink`). Like the reference
/// `mruby` (`src/print.c`): on a terminal each write is flushed at once, so a program that
/// draws and then waits is seen drawing; anywhere else it is block buffered, flushed when the
/// buffer fills and by [`flush_output`] before the command reports an error or ends.
fn write_output(bytes: &[u8]) {
    static TERMINAL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *TERMINAL.get_or_init(|| std::io::IsTerminal::is_terminal(&std::io::stdout())) {
        let mut out = std::io::stdout().lock();
        out.write_all(bytes).ok();
        out.flush().ok();
        return;
    }
    OUTPUT.with(|b| {
        let mut b = b.borrow_mut();
        b.extend_from_slice(bytes);
        if b.len() >= OUTPUT_BLOCK { std::io::stdout().lock().write_all(&b).ok(); b.clear(); }
    });
}

const OUTPUT_BLOCK: usize = 64 * 1024;
thread_local! {
    static OUTPUT: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Writes out what [`write_output`] holds and anything left in the VM's own buffer.
fn flush_output(vm: &mut sabiruby::Vm) {
    let mut out = std::io::stdout().lock();
    OUTPUT.with(|b| { let mut b = b.borrow_mut(); out.write_all(&b).ok(); b.clear(); });
    out.write_all(vm.take_output().as_slice()).ok();
    out.flush().ok();
}

fn clock_ns() -> u64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
}

const COPYRIGHT: &str = "sabiruby - Copyright (c) 2026 kishima, MIT\n\
                         mruby - Copyright (c) 2010- mruby developers (the compiler and mrblib)";

#[derive(Parser)]
#[command(
    name = "sabiruby",
    about = "Run Ruby on the SabiRuby VM (mruby 4.1 bytecode)",
    long_about = "Runs Ruby source or mruby bytecode (.mrb) on the SabiRuby VM.\n\
                  The switches follow the reference `mruby` command; `compile` is `mrbc`.\n\
                  With no program file and no -e, the program is read from standard input.\n\
                  SABIRUBY_GC_STRESS=1 collects at every instruction boundary after an allocation.",
    disable_version_flag = true,
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    /// load and execute RiteBinary (mrb) file
    #[arg(short = 'b')]
    bytecode: bool,
    /// check syntax only
    #[arg(short = 'c')]
    check: bool,
    /// set debugging flags (set $DEBUG to true)
    #[arg(short = 'd')]
    debug: bool,
    /// one line of script (may be given more than once)
    #[arg(short = 'e', value_name = "command")]
    commands: Vec<String>,
    /// load the library before executing your script
    #[arg(short = 'r', value_name = "library")]
    requires: Vec<String>,
    /// print version number, then run in verbose mode
    #[arg(short = 'v')]
    version_verbose: bool,
    /// run in verbose mode (print the instruction listing before running)
    #[arg(long)]
    verbose: bool,
    /// print the version
    #[arg(long)]
    version: bool,
    /// print the copyright
    #[arg(long)]
    copyright: bool,
    /// print instructions, time and GC statistics to stderr
    #[arg(long)]
    stats: bool,
    /// go on with a checkpoint image written by C mruby (feature `checkpoint`)
    #[cfg(feature = "checkpoint")]
    #[arg(long, value_name = "image")]
    resume: Option<String>,
    /// read the writer's permanent names (its `checkpoint names`, one per line) and print
    /// what each one is here, or why it has nothing here (feature `checkpoint`)
    #[cfg(feature = "checkpoint")]
    #[arg(long, value_name = "file")]
    checkpoint_names: Option<String>,
    #[arg(value_name = "programfile")]
    program: Option<String>,
    /// arguments for the program (ARGV)
    #[arg(value_name = "arguments", trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run a program (`sabiruby run foo.rb` = `sabiruby foo.rb`)
    Run {
        #[arg(long)]
        stats: bool,
        programfile: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compile Ruby source to a RITE binary, as `mrbc`
    Compile {
        /// place the output into `<outfile>` ("-" for standard output)
        #[arg(short = 'o', value_name = "outfile")]
        output: Option<String>,
        /// produce debugging information (line numbers, local variable names)
        #[arg(short = 'g')]
        debug_info: bool,
        /// check syntax only
        #[arg(short = 'c')]
        check: bool,
        /// remove local variables
        #[arg(long)]
        remove_lv: bool,
        /// prohibit using OP_EXTs
        #[arg(long)]
        no_ext_ops: bool,
        /// disable peephole optimization
        #[arg(long)]
        no_optimize: bool,
        programfile: String,
    },
    /// Print the instruction listing of a program (.rb or .mrb)
    Dump { programfile: String },
    /// Run mruby's test suite and print a Markdown report
    Mrbtest {
        /// print the message of every assertion
        #[arg(short = 'v')]
        verbose: bool,
        #[arg(required = true, num_args = 2..)]
        files: Vec<String>,
    },
    /// Run one test file under mruby/edge's compatibility driver (its `helpers.rb`, compiled);
    /// the exit status is 0 only if every assertion passed (`tools/edge_compare.sh`)
    EdgeSpec {
        /// append the driver's events (JSON lines) to this file
        #[arg(long, value_name = "file")]
        events: Option<String>,
        /// the name the events give the test file (default: its own, with `.rb`)
        #[arg(long, value_name = "name")]
        label: Option<String>,
        /// also define the ports of the reference test driver's C fixtures that `mrbtest`
        /// defines (`TestNotImplement`, the `__env_*` helpers, `TestVFormat`, `AryShared`, ...);
        /// mruby/edge has none of them, so a run with them is not under its conditions
        #[arg(long)]
        with_fixtures: bool,
        helpers: String,
        test: String,
    },
}

fn version_line() -> String {
    format!("sabiruby {} (mruby 4.1 bytecode, RITE 0400), compiler: {}", env!("CARGO_PKG_VERSION"), sabiruby_compiler::version())
}

/// Compiles Ruby source with the reference compiler; errors print as `FILE:LINE:COL: message`.
fn compile_source(src: &[u8], opts: &sabiruby_compiler::Options) -> Result<Vec<u8>, ExitCode> {
    sabiruby_compiler::compile(src, opts).map_err(|e| { eprintln!("{e}"); ExitCode::from(1) })
}

fn read_file(path: &str) -> Result<Vec<u8>, ExitCode> {
    std::fs::read(path).map_err(|_| { eprintln!("sabiruby: Cannot open program file: {path}"); ExitCode::from(1) })
}

/// A program: a RITE binary as is, anything else compiled as Ruby source (with the debug
/// information, which `local_variables` and `Proc#parameters` need).
fn load_program(path: &str, bytecode_only: bool) -> Result<Vec<u8>, ExitCode> {
    let bytes = read_file(path)?;
    if bytes.starts_with(b"RITE") { return Ok(bytes); }
    if bytecode_only { eprintln!("sabiruby: Cannot load RiteBinary: {path}"); return Err(ExitCode::from(1)); }
    compile_source(&bytes, &sabiruby_compiler::Options { filename: path.to_string(), debug_info: true, ..Default::default() })
}

fn compile_cmd(programfile: String, output: Option<String>, debug_info: bool, check: bool,
               remove_lv: bool, no_ext_ops: bool, no_optimize: bool) -> ExitCode {
    let src = match read_file(&programfile) { Ok(b) => b, Err(code) => return code };
    let opts = sabiruby_compiler::Options { filename: programfile.clone(), debug_info, remove_lv, no_ext_ops, no_optimize };
    let bin = match compile_source(&src, &opts) { Ok(b) => b, Err(code) => return code };
    if check {
        println!("Syntax OK");
        return ExitCode::SUCCESS;
    }
    // as mrbc: the extension is replaced; a name without one gets `.mrb` added here (mrbc
    // would reuse the input name and overwrite the source)
    let output = output.unwrap_or_else(|| std::path::Path::new(&programfile).with_extension("mrb").to_string_lossy().into_owned());
    let written = if output == "-" { std::io::stdout().write_all(&bin) } else { std::fs::write(&output, &bin) };
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => { eprintln!("sabiruby: cannot write {output}: {e}"); ExitCode::from(1) }
    }
}

fn dump(bin: &[u8]) -> ExitCode {
    match sabiruby::rite::parse(bin) {
        Ok(rite) => { print!("{}", sabiruby::vm::dump(&rite)); ExitCode::SUCCESS }
        Err(e) => { eprintln!("{e}"); ExitCode::from(1) }
    }
}

/// The directory of the program and the working directory, which is what `require` searches.
fn program_load_path(programfile: &str) -> Vec<String> {
    match std::path::Path::new(programfile).parent().map(|d| d.to_string_lossy().into_owned()) {
        Some(d) if !d.is_empty() && d != "." => vec![d, String::from(".")],
        _ => vec![String::from(".")],
    }
}

/// What the reference `mruby` writes to standard error for an exception nothing caught
/// (`mrb_print_error` → `print_backtrace`, src/backtrace.c): "trace (most recent call last):",
/// the backtrace from the outermost frame down to the second entry as `\t[i] line`, then the
/// first entry, ": " and `mrb_exc_get_output` (src/error.c) — the message and the class in
/// parentheses, or the class alone when the message is nil or empty. An exception with no
/// backtrace (`set_backtrace([])`) starts with `(unknown):0: ` and no header. The backtrace is
/// the one `Exception#backtrace` of the core class keeps (raise's record, or what
/// `set_backtrace` was given), not a method the program redefined, as the reference reads the
/// field. A failure that is not a Ruby exception is described as before.
fn uncaught_report(vm: &mut sabiruby::Vm, e: &sabiruby::VmError) -> Vec<u8> {
    let exc = match e {
        sabiruby::VmError::Raise(v @ sabiruby::Value::Obj(_)) => *v,
        _ => return vm.describe_error(e).into_bytes(),
    };
    let lines: Vec<Option<Vec<u8>>> = {
        let m = vm.intern("backtrace");
        let bt = match vm.find_method(vm.core.exception, m) {
            Some((sabiruby::object::Method::Native(f, _), _)) => f(vm, exc, &[], sabiruby::Value::Nil).unwrap_or(sabiruby::Value::Nil),
            _ => sabiruby::Value::Nil,
        };
        let items: Vec<sabiruby::Value> = vm.ary_vals(bt).unwrap_or_default();
        items.into_iter().map(|v| vm.str_bytes(v).map(|b| b.to_vec())).collect()
    };
    let mut out = Vec::new();
    if lines.is_empty() {
        out.extend_from_slice(b"(unknown):0: ");
    } else {
        out.extend_from_slice(b"trace (most recent call last):\n");
        for i in (1..lines.len()).rev() {
            if let Some(l) = &lines[i] { out.extend_from_slice(format!("\t[{i}] ").as_bytes()); out.extend_from_slice(l); out.push(b'\n'); }
        }
        if let Some(l) = &lines[0] { out.extend_from_slice(l); out.extend_from_slice(b": "); }
    }
    let cls = vm.real_class_of(exc);
    let cname = vm.class_name(cls);
    let mesg = match exc.obj() { Some(o) => vm.heap.ivar_get(o, vm.s.mesg), None => sabiruby::Value::Nil };
    // the reference keeps the message as a String (`exc_initialize` converts it); here it is
    // kept as given, so it is converted as `Exception#to_s` converts it
    let mesg: Vec<u8> = if mesg.is_nil() { Vec::new() } else { vm.as_string(mesg).unwrap_or_default() };
    if mesg.is_empty() { out.extend_from_slice(cname.as_bytes()); } else {
        out.extend_from_slice(&mesg);
        out.extend_from_slice(format!(" ({cname})").as_bytes());
    }
    out.push(b'\n');
    out
}

/// What runs: a program from its start, or (feature `checkpoint`) an image from where it stopped.
enum Start<'a> {
    Program(&'a [u8]),
    #[cfg(feature = "checkpoint")]
    Image(&'a [u8]),
}

/// Runs a RITE binary with `ARGV` = `argv` and `$DEBUG` = `debug`. `load_path` becomes
/// `$LOAD_PATH` (where `require` looks) and `requires` are the `-r` libraries, loaded first.
fn run(bin: &[u8], argv: &[String], stats: bool, debug: bool, load_path: &[String], requires: &[String]) -> ExitCode {
    run_start(Start::Program(bin), argv, stats, debug, load_path, requires)
}

/// A VM as `run` makes it, before the program: mrblib, ARGV, `$DEBUG`, the load path.
fn new_vm(argv: &[String], stats: bool, debug: bool, load_path: &[String]) -> Result<sabiruby::Vm, ExitCode> {
    let mut vm = sabiruby::Vm::new();
    vm.set_gc_stress(gc_stress());
    // `eval` (and later `require`) compiles through the reference compiler of this build
    vm.set_host(Box::new(sabiruby_compiler::Compiler::new()));
    if stats { vm.gc_clock = Some(clock_ns); }
    vm.wall_clock = Some(wall_clock);
    vm.sleep_hook = Some(|micros| std::thread::sleep(std::time::Duration::from_micros(micros)));
    vm.output_sink = Some(write_output);
    if let Err(e) = vm.load_mrblib() { eprintln!("failed to initialize VM (mrblib): {}", vm.describe_error(&e)); return Err(ExitCode::from(1)); }
    // like the `mruby` command: ARGV holds the arguments after the program file
    let argv: Vec<sabiruby::Value> = argv.iter().map(|a| vm.str_new(a.as_bytes())).collect();
    let argv = vm.ary_new(argv);
    let n = vm.intern("ARGV");
    vm.heap.class_mut(vm.core.object).consts.insert(n, sabiruby::value::Slot::from(argv));
    let d = vm.intern("$DEBUG");
    vm.globals.insert(d, sabiruby::value::Slot::from(sabiruby::Value::bool(debug)));
    // where `require` looks: the directory of the program and the working directory
    let paths: Vec<&str> = load_path.iter().map(|p| p.as_str()).collect();
    vm.set_load_path(&paths);
    Ok(vm)
}

fn run_start(start: Start, argv: &[String], stats: bool, debug: bool, load_path: &[String], requires: &[String]) -> ExitCode {
    let mut vm = match new_vm(argv, stats, debug, load_path) { Ok(vm) => vm, Err(code) => return code };
    // `-r lib`: the reference `mruby` loads each library file before the program
    for lib in requires {
        let path = vm.str_new(lib.as_bytes());
        let m = vm.intern("load");
        let recv = sabiruby::Value::Obj(vm.top_self);
        if let Err(e) = vm.funcall(recv, m, &[path], sabiruby::Value::Nil) {
            flush_output(&mut vm);
            let report = uncaught_report(&mut vm, &e);
            std::io::stderr().write_all(&report).ok();
            return ExitCode::from(1);
        }
    }
    let (gc0, gct0) = (vm.gc_count, vm.gc_time_ns);
    let started = std::time::Instant::now();
    let result = match start {
        Start::Program(bin) => vm.load_and_run(bin),
        #[cfg(feature = "checkpoint")]
        Start::Image(img) => vm.checkpoint_resume(img),
    };
    if stats {
        let e = started.elapsed();
        // before the instructions line: tools/bench.sh reads the last line
        eprintln!("gc: {} collections, {:.3} ms, live: {}, heap slots: {}", vm.gc_count - gc0, (vm.gc_time_ns - gct0) as f64 / 1e6, vm.heap.live_count(), vm.heap.len());
        eprintln!("instructions: {} elapsed_ms: {:.3} ns_per_instruction: {:.1}", vm.instructions, e.as_secs_f64() * 1000.0, e.as_nanos() as f64 / vm.instructions.max(1) as f64);
    }
    flush_output(&mut vm);
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            let report = uncaught_report(&mut vm, &e);
            std::io::stderr().write_all(&report).ok();
            ExitCode::from(1)
        }
    }
}

/// `sabiruby [switches] [programfile] [arguments]`, as the reference `mruby` command.
fn program(cli: Cli) -> ExitCode {
    if cli.copyright {
        println!("{COPYRIGHT}");
        return ExitCode::SUCCESS;
    }
    if cli.version || cli.version_verbose {
        println!("{}", version_line());
        if cli.version { return ExitCode::SUCCESS; }
    }
    #[cfg(feature = "checkpoint")]
    if let Some(path) = cli.checkpoint_names.as_deref() {
        return checkpoint_names(path);
    }
    #[cfg(feature = "checkpoint")]
    if let Some(path) = cli.resume.as_deref() {
        let img = match read_file(path) { Ok(b) => b, Err(code) => return code };
        // `-r lib` loads libraries before the image is restored (a host's own setup)
        return run_start(Start::Image(&img), &cli.args, cli.stats, cli.debug, &[String::from(".")], &cli.requires);
    }
    // where `require` looks: the program's own directory first, then the working directory
    let load_path = match cli.program.as_deref().filter(|_| cli.commands.is_empty()) {
        Some(p) => program_load_path(p),
        None => vec![String::from(".")],
    };
    // -e wins over a program file, which then becomes the first argument (as mruby)
    let (bin, argv) = if !cli.commands.is_empty() {
        let src = cli.commands.join("\n");
        let opts = sabiruby_compiler::Options { filename: "-e".into(), debug_info: true, ..Default::default() };
        let argv: Vec<String> = cli.program.into_iter().chain(cli.args).collect();
        match compile_source(src.as_bytes(), &opts) { Ok(bin) => (bin, argv), Err(code) => return code }
    } else if let Some(path) = cli.program.clone() {
        match load_program(&path, cli.bytecode) { Ok(bin) => (bin, cli.args), Err(code) => return code }
    } else {
        // no program file: read it from standard input, as mruby does
        let mut src = Vec::new();
        if let Err(e) = std::io::stdin().read_to_end(&mut src) { eprintln!("sabiruby: cannot read standard input: {e}"); return ExitCode::from(1); }
        if src.starts_with(b"RITE") { (src, cli.args) } else {
            let opts = sabiruby_compiler::Options { filename: "-".into(), debug_info: true, ..Default::default() };
            match compile_source(&src, &opts) { Ok(bin) => (bin, cli.args), Err(code) => return code }
        }
    };
    if cli.check {
        println!("Syntax OK");
        return ExitCode::SUCCESS;
    }
    if cli.verbose || cli.version_verbose {
        let code = dump(&bin);
        if code != ExitCode::SUCCESS { return code; }
    }
    run(&bin, &argv, cli.stats, cli.debug, &load_path, &cli.requires)
}

/// `--checkpoint-names FILE`: one line per name: `ok <name> <what>` or `missing <name> <why>`,
/// then a count per prefix (`M:`, `F:`, classes, ...).
#[cfg(feature = "checkpoint")]
fn checkpoint_names(path: &str) -> ExitCode {
    let text = match std::fs::read_to_string(path) { Ok(t) => t, Err(e) => { eprintln!("sabiruby: {path}: {e}"); return ExitCode::from(2); } };
    let mut vm = match new_vm(&[], false, false, &[String::from(".")]) { Ok(vm) => vm, Err(code) => return code };
    let mut counts: std::collections::BTreeMap<String, (usize, usize)> = std::collections::BTreeMap::new();
    for name in text.lines().filter(|l| !l.is_empty()) {
        let prefix = match name.find(':') { Some(i) if i <= 1 => name[..=i].to_string(), _ => String::from("class") };
        let e = counts.entry(prefix).or_default();
        match vm.checkpoint_name(name) {
            Ok(what) => { e.0 += 1; println!("ok {name} {what}"); }
            Err(why) => { e.1 += 1; println!("missing {name} {why}"); }
        }
    }
    for (p, (ok, missing)) in &counts { println!("total {p} {ok} found {missing} missing"); }
    ExitCode::SUCCESS
}

/// `sabiruby mrbtest [-v] assert.mrb t/*.mrb`: one fresh VM per file, a
/// Markdown table of the `report` counts, and the opcodes never executed.
fn mrbtest(verbose: bool, files: &[String]) -> ExitCode {
    let assert_mrb = match std::fs::read(&files[0]) { Ok(b) => b, Err(e) => { eprintln!("{}: {}", files[0], e); return ExitCode::from(1); } };
    // a file named `prelude.mrb` is not a test but the helpers the gem test files share, which
    // the reference's driver has by linking every file into one program (`tools/mrbtest.sh`)
    let prelude = files[1..].iter()
        .find(|f| std::path::Path::new(f).file_stem().map(|s| s == "prelude").unwrap_or(false))
        .and_then(|f| std::fs::read(f).ok())
        .unwrap_or_default();
    let cap: u64 = 300_000_000;
    let mut rows = Vec::new();
    let mut counts = vec![0u64; sabiruby::opcode::OP_COUNT];
    let mut tot = sabiruby::mrbtest::Summary::default();
    for f in &files[1..] {
        let bin = match std::fs::read(f) { Ok(b) => b, Err(e) => { eprintln!("{f}: {e}"); return ExitCode::from(1); } };
        let name = std::path::Path::new(f).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if name == "prelude" { continue; }
        let host = Box::new(sabiruby_compiler::Compiler::new());
        let sum = match sabiruby::mrbtest::run_file_prelude(&assert_mrb, &prelude, &bin, cap, verbose, gc_stress(), Some(host)) {
            Ok(s) => s,
            Err(e) => { eprintln!("{name}: {e}"); return ExitCode::from(1); }
        };
        for (i, c) in sum.op_counts.iter().enumerate() { counts[i] += c; }
        tot.total += sum.total; tot.ok += sum.ok; tot.ko += sum.ko; tot.crash += sum.crash; tot.warn += sum.warn; tot.skip += sum.skip;
        if verbose {
            eprintln!("=== {name}");
            eprintln!("{}", String::from_utf8_lossy(&sum.output));
        }
        let note = match (&sum.aborted, sum.timeout) {
            (Some(a), _) => format!("aborted: {}", a.lines().next().unwrap_or("")),
            (None, true) => "timeout".to_string(),
            _ => String::new(),
        };
        rows.push((name, sum, note));
    }
    println!("| file | total | ok | ko | crash | warn | skip | note |");
    println!("|---|---:|---:|---:|---:|---:|---:|---|");
    for (name, s, note) in &rows {
        println!("| {} | {} | {} | {} | {} | {} | {} | {} |", name, s.total, s.ok, s.ko, s.crash, s.warn, s.skip, note);
    }
    println!("| **all** | {} | {} | {} | {} | {} | {} | |", tot.total, tot.ok, tot.ko, tot.crash, tot.warn, tot.skip);
    let never = sabiruby::mrbtest::unexecuted_opcodes(&counts);
    println!();
    println!("Opcodes executed: {} / {} (never: {})", sabiruby::opcode::OP_COUNT - never.len(), sabiruby::opcode::OP_COUNT, never.join(", "));
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    // Native -> VM re-entry recurses on the host stack; give it room.
    let t = std::thread::Builder::new().stack_size(256 << 20).spawn(real_main).expect("spawn");
    t.join().unwrap_or(ExitCode::from(1))
}

fn real_main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        None => program(cli),
        Some(Command::Run { stats, programfile, args }) => {
            match load_program(&programfile, false) { Ok(bin) => { let lp = program_load_path(&programfile); run(&bin, &args, stats, false, &lp, &[]) } Err(code) => code }
        }
        Some(Command::Compile { output, debug_info, check, remove_lv, no_ext_ops, no_optimize, programfile }) =>
            compile_cmd(programfile, output, debug_info, check, remove_lv, no_ext_ops, no_optimize),
        Some(Command::Dump { programfile }) => {
            match load_program(&programfile, false) { Ok(bin) => dump(&bin), Err(code) => code }
        }
        Some(Command::Mrbtest { verbose, files }) => mrbtest(verbose, &files),
        Some(Command::EdgeSpec { events, label, with_fixtures, helpers, test }) => edge_spec::run(&helpers, &test, events, label, with_fixtures, wall_clock),
    }
}

//! The reference mruby 4.1.0-rc compiler as a Rust library: Ruby source in, RITE bytecode out.
//!
//! This crate does not reimplement the compiler. It builds mruby 4.1.0-rc's own
//! `mruby-compiler` (the Prism parser plus mruby's code generator) as C, standalone like
//! the reference `mrbc`, and calls it through a small C shim. The output is byte-for-byte
//! what `mrbc` writes for the same source and options (checked by the golden tests of the
//! repository). The bytecode runs on the [SabiRuby](https://crates.io/crates/sabiruby) VM,
//! or on mruby itself.
//!
//! ```
//! let bin = sabiruby_compiler::compile(b"p 1 + 2", &Default::default()).unwrap();
//! assert_eq!(&bin[..4], b"RITE");
//!
//! let err = sabiruby_compiler::compile(b"def", &Default::default()).unwrap_err();
//! assert_eq!(err.diagnostics[0].kind, sabiruby_compiler::Kind::ParserError);
//! ```
//!
//! Needs a C compiler at build time (the `cc` crate). For `wasm32-wasip1`, use wasi-sdk's clang
//! (`CC_wasm32_wasip1`); the module then needs WebAssembly exception handling (see the README).
//! The vendored sources and their licences are listed in `vendor/VENDOR.md`.

use std::ffi::CString;
use std::fmt;
use std::sync::Mutex;

mod ffi;

/// Compiler options; the fields mirror `mrbc`'s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Name recorded in the debug info and used in diagnostics (`mrbc` uses the path given).
    pub filename: String,
    /// `-g`: include the DBG section (line numbers).
    pub debug_info: bool,
    /// `--remove-lv`: drop the LVAR section (local variable names).
    pub remove_lv: bool,
    /// `--no-ext-ops`: do not emit `OP_EXT1..3`.
    pub no_ext_ops: bool,
    /// `--no-optimize`: disable peephole optimisation.
    pub no_optimize: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { filename: "-e".into(), debug_info: false, remove_lv: false, no_ext_ops: false, no_optimize: false }
    }
}

/// Kind of a diagnostic (`mrc_diagnostic_code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    ParserWarning,
    ParserError,
    GeneratorWarning,
    GeneratorError,
}

impl Kind {
    pub fn is_error(self) -> bool {
        matches!(self, Kind::ParserError | Kind::GeneratorError)
    }
}

/// A message from the parser or the code generator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: Kind,
    pub message: String,
    pub filename: String,
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for Diagnostic {
    /// `FILE:LINE:COL: message`, as `mrbc` prints it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}: {}", self.filename, self.line, self.column, self.message)
    }
}

/// Compilation failed. `diagnostics` holds the errors, and the warnings reported with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub diagnostics: Vec<Diagnostic>,
}

impl fmt::Display for CompileError {
    /// The error diagnostics, one per line (warnings are left out, as `mrbc` does).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for d in self.diagnostics.iter().filter(|d| d.kind.is_error()) {
            if !first { writeln!(f)?; }
            write!(f, "{d}")?;
            first = false;
        }
        if first { write!(f, "compile error")?; }
        Ok(())
    }
}

impl std::error::Error for CompileError {}

/// `mrc_presym.c` keeps a global that every parse writes, so compilations are serialised.
static LOCK: Mutex<()> = Mutex::new(());

/// Compiles Ruby source to a RITE binary (what `mrbc` would write to the `.mrb` file).
pub fn compile(src: &[u8], opts: &Options) -> Result<Vec<u8>, CompileError> {
    let internal = |message: &str| CompileError { diagnostics: vec![Diagnostic { kind: Kind::GeneratorError, message: message.into(), filename: opts.filename.clone(), line: 0, column: 0 }] };
    let filename = CString::new(opts.filename.as_str()).map_err(|_| internal("file name contains a NUL byte"))?;
    let mut flags = 0;
    if opts.debug_info { flags |= ffi::DEBUG_INFO; }
    if opts.remove_lv { flags |= ffi::REMOVE_LV; }
    if opts.no_ext_ops { flags |= ffi::NO_EXT_OPS; }
    if opts.no_optimize { flags |= ffi::NO_OPTIMIZE; }
    let (code, bin, diag) = {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        ffi::compile(src, &filename, flags)
    };
    match code {
        ffi::OK => Ok(bin),
        ffi::COMPILE_ERROR => {
            let diagnostics = parse_diagnostics(&diag);
            if diagnostics.is_empty() { Err(internal("compile error")) } else { Err(CompileError { diagnostics }) }
        }
        ffi::DUMP_ERROR => Err(internal("could not write the RITE binary")),
        ffi::NO_MEMORY => Err(internal("out of memory")),
        _ => Err(internal("unexpected result from the compiler")),
    }
}

/// Compiles an `eval` string: like [`compile`], plus the line it starts at and the local
/// variable names of the enclosing scopes, so the string can read and write them. This is
/// the only entry that uses the one change made to the vendored compiler
/// (`SABIRUBY_EVAL_SCOPES`, `vendor/VENDOR.md`); [`compile`] is untouched by it.
///
/// `scopes[0]` is the caller and each next one is further out; within a scope the names are
/// in the order of the irep's local variable table, an empty name standing for a hole (an
/// unnamed parameter) so that a position is a register index.
pub fn compile_eval(src: &[u8], opts: &Options, line: u32, scopes: &[Vec<Vec<u8>>]) -> Result<Vec<u8>, CompileError> {
    let internal = |message: &str| CompileError { diagnostics: vec![Diagnostic { kind: Kind::GeneratorError, message: message.into(), filename: opts.filename.clone(), line: 0, column: 0 }] };
    let filename = CString::new(opts.filename.as_str()).map_err(|_| internal("file name contains a NUL byte"))?;
    let mut flags = 0;
    if opts.debug_info { flags |= ffi::DEBUG_INFO; }
    // the blob `csrc/shim.c` reads: per scope a u32 count, then u16-prefixed names
    let mut blob: Vec<u8> = Vec::new();
    for scope in scopes {
        blob.extend_from_slice(&(scope.len() as u32).to_le_bytes());
        for name in scope {
            let n = name.len().min(u16::MAX as usize);
            blob.extend_from_slice(&(n as u16).to_le_bytes());
            blob.extend_from_slice(&name[..n]);
        }
    }
    let (code, bin, diag) = {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        ffi::compile_eval(src, &filename, line, flags, &blob, scopes.len() as u32)
    };
    match code {
        ffi::OK => Ok(bin),
        ffi::COMPILE_ERROR => {
            let diagnostics = parse_diagnostics(&diag);
            if diagnostics.is_empty() { Err(internal("compile error")) } else { Err(CompileError { diagnostics }) }
        }
        ffi::DUMP_ERROR => Err(internal("could not write the RITE binary")),
        ffi::NO_MEMORY => Err(internal("out of memory")),
        _ => Err(internal("unexpected result from the compiler")),
    }
}

/// The [`Host`](sabiruby::Host) the VM asks to compile an `eval` string, and to read a file
/// for `require`. Install it with `vm.set_host(Box::new(Compiler::new()))`; the dependency
/// points this way (compiler → VM), so the VM crate stays free of C.
///
/// `read_file` reads the file system, which is what the `sabiruby` command wants; a host with
/// another idea of where files come from (a browser, a game's assets) writes its own `Host`
/// and can still call [`compile_eval`] for the compiling half.
#[cfg(feature = "host")]
#[derive(Debug, Default)]
pub struct Compiler {
    _private: (),
}

#[cfg(feature = "host")]
impl Compiler {
    pub fn new() -> Compiler {
        Compiler { _private: () }
    }
}

#[cfg(feature = "host")]
impl sabiruby::Host for Compiler {
    fn compile(&mut self, src: &[u8], opts: &sabiruby::EvalOptions) -> Result<Vec<u8>, String> {
        let o = Options { filename: opts.filename.into(), debug_info: opts.debug_info, ..Default::default() };
        compile_eval(src, &o, opts.line, opts.scopes).map_err(|e| {
            // the message the reference's eval puts in the SyntaxError: the first error,
            // without the file and line it prefixes itself
            match e.diagnostics.iter().find(|d| d.kind.is_error()) {
                Some(d) => d.message.clone(),
                None => "compile error".into(),
            }
        })
    }
    fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }
    fn file_exists(&mut self, path: &str) -> bool {
        std::path::Path::new(path).is_file()
    }
}

fn parse_diagnostics(text: &str) -> Vec<Diagnostic> {
    text.split('\u{1e}')
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            let mut f = r.splitn(5, '\u{1f}');
            let kind = match f.next()? { "0" => Kind::ParserWarning, "1" => Kind::ParserError, "2" => Kind::GeneratorWarning, _ => Kind::GeneratorError };
            let line = f.next()?.parse().unwrap_or(0);
            let column = f.next()?.parse().unwrap_or(0);
            let filename = f.next()?.to_string();
            let message = f.next().unwrap_or("").to_string();
            Some(Diagnostic { kind, message, filename, line, column })
        })
        .collect()
}

/// Prism's syntax tree of `src`, pretty-printed (`pm_prettyprint`): the tree mruby's code
/// generator walks, in the format a debug build of `mrbc --verbose` prints. Parsed as `compile`
/// parses (the file name shows in `SourceFileNode`); a source with syntax errors still gets a
/// tree (with missing nodes), the errors come from `compile`. Needs the feature `ast`.
#[cfg(feature = "ast")]
pub fn ast(src: &[u8], filename: &str) -> Option<String> {
    let filename = CString::new(filename).ok()?;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    ffi::ast(src, &filename)
}

/// One category byte per byte of `src`, for colouring an editor: `highlight(src).len() ==
/// src.len()` and every byte is 0..=8.
///
/// | | | | |
/// |---|---|---|---|
/// | 0 | default | 3 | comment | 6 | constant |
/// | 1 | keyword | 4 | number | 7 | variable (`@ivar`, `@@cvar`, `$gvar`) |
/// | 2 | string | 5 | symbol | 8 | method name (after `def`, `.` or `&.`) |
///
/// The categories come from Prism's lexer (`csrc/shim.c`), so a `#{...}` inside a string, a
/// regular expression and a heredoc are told apart the way the parser tells them apart, and
/// **a source that does not parse still gets a map** -- the lexer recovers, and an editor's
/// text is broken most of the time it is looked at. Token boundaries are character
/// boundaries, so a run of one category never cuts a UTF-8 character in half.
///
/// ```
/// let map = sabiruby_compiler::highlight("def leaf\n  @n = 1 # ha\nend".as_bytes());
/// assert_eq!(map[0..3], [1, 1, 1]);   // `def` is a keyword
/// assert_eq!(map[4..8], [8, 8, 8, 8]); // `leaf` is the method name
/// ```
///
/// There is no maximum source size; `csrc/shim.c` says why.
pub fn highlight(src: &[u8]) -> Vec<u8> {
    // No LOCK: this reaches Prism only (`pm_parse`), and the global that makes a compilation
    // serial is `vendor/mruby-compiler/src/mrc_presym.c`'s, which nothing under
    // `vendor/prism` touches. An editor may colour while another thread compiles.
    ffi::highlight(src)
}

/// The compiler this crate embeds, e.g. `"mruby 4.1.0-rc (3cf73ee), Prism 1.9.0"`.
pub fn version() -> &'static str {
    ffi::version()
}

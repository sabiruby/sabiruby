//! `highlight()`: one category byte per source byte, for an editor's colours.
//!
//! Every expectation here was read off Prism 1.9.0 (the vendored version) before it was
//! written down, never guessed. The maps are spelled as digit strings so the file doubles as
//! the record of what the two passes actually answer for interpolation, regular expressions,
//! labels, heredocs, a Japanese comment, the garden's own bare calls and a source that does
//! not parse.

use sabiruby_compiler::highlight;

const DEFAULT: u8 = 0;
const KEYWORD: u8 = 1;
const STRING: u8 = 2;
const COMMENT: u8 = 3;
const NUMBER: u8 = 4;
const SYMBOL: u8 = 5;
const VARIABLE: u8 = 7;
const METHOD: u8 = 8;

/// The map of `src` as one digit per byte, so a failure prints the whole line.
fn map(src: &str) -> String {
    let m = highlight(src.as_bytes());
    assert_eq!(m.len(), src.len(), "the map is as long as the source");
    assert!(m.iter().all(|&c| c <= METHOD), "every byte is a category 0..=8");
    m.iter().map(|c| char::from(b'0' + c)).collect()
}

/// The categories of the bytes of `needle`, which must occur exactly once in `src`.
fn of(src: &str, needle: &str) -> Vec<u8> {
    let at = src.find(needle).unwrap_or_else(|| panic!("{needle:?} is not in {src:?}"));
    assert_eq!(src.rfind(needle), Some(at), "{needle:?} occurs more than once");
    highlight(src.as_bytes())[at..at + needle.len()].to_vec()
}

#[test]
fn a_creature_file() {
    // the shape of garden/ruby/creatures/beetle.rb: a method, an instance variable, a number,
    // a comment. The newline that ends a comment belongs to the comment's token (measured).
    let src = "def leaf\n  @n = 1 # ha\nend\n";
    assert_eq!(map(src), "111088880007700040333331110");
    assert_eq!(of(src, "def"), [KEYWORD; 3]);
    assert_eq!(of(src, "leaf"), [METHOD; 4]);
    assert_eq!(of(src, "@n"), [VARIABLE; 2]);
    assert_eq!(of(src, "# ha"), [COMMENT; 4]);
    assert_eq!(of(src, "end"), [KEYWORD; 3]);
}

#[test]
fn the_garden_scripts_bare_calls() {
    // what the garden's scripts are mostly made of: a call with neither a receiver nor
    // parentheses. Nothing in the token stream says these are calls; the tree does.
    assert_eq!(map("tell :all, \"season\", s"), "8888055550022222222000");
    assert_eq!(of("tell :all, \"season\", s", "tell"), [METHOD; 4]);
    assert_eq!(of("tell :all, \"season\", s", ":all"), [SYMBOL; 4]); // the whole symbol
    assert_eq!(map("sleep 0.5"), "888880444");
    assert_eq!(map("every 60 do\nend"), "888880440110111");
    assert_eq!(map("day_length 60.0"), "888888888804444");
    assert_eq!(map("world do\nend"), "888880110111");
    assert_eq!(map("plants.each do |p|\nend"), "0000000888801100000111");
    // `me[:Hunger]`: the `[` and `]` of the index call are the method name, and the symbol
    // inside is repainted by the child node
    assert_eq!(map("me[:Hunger]"), "00855555558");
}

#[test]
fn method_names_come_from_the_tree() {
    assert_eq!(map("a.b(1).c"), "00804008");
    assert_eq!(map("p 1"), "804");
    assert_eq!(map("p x"), "800");
    // ... but a bare identifier that is only a call because there is no local variable is
    // left alone (PM_CALL_NODE_FLAGS_VARIABLE_CALL), so `x` reads as the variable it looks like
    assert_eq!(map("x = 1\np x"), "000040800");
    // the name being defined, operators included
    assert_eq!(map("def ==(o)\nend"), "1110880000111");
    assert_eq!(map("def [](i)\nend"), "1110880000111");
    assert_eq!(map("def foo=(v)\nend"), "111088880000111");
    // an operator call is a call: `=~` is the method name of `x =~ /re/`, `+` of `a + b`
    assert_eq!(of("x =~ /re/", "=~"), [METHOD; 2]);
    // the receiver and its `.` may be on lines of their own, with a comment between
    assert_eq!(map("obj\n  .foo\n  # c\n  .bar"), "00000008880003333000888");
    assert_eq!(map("a&.b"), "0008");
    // a method name is not always ASCII
    assert_eq!(map("あ.い"), "0000888");
}

#[test]
fn interpolation_paints_the_punctuation_and_not_the_code() {
    let src = "x = \"a #{b} c\"";
    assert_eq!(map(src), "00002222202222");
    assert_eq!(of(src, "#{"), [STRING; 2]);
    assert_eq!(of(src, "b"), [DEFAULT]); // what is inside is ordinary code
    assert_eq!(of(src, "}"), [STRING]);
}

#[test]
fn a_regular_expression_is_a_string() {
    // measured, not assumed: REGEXP_BEGIN and REGEXP_END are category 2, like the content
    let src = "x =~ /re/";
    assert_eq!(map(src), "008802222");
    assert_eq!(of(src, "/re/"), [STRING; 4]);
}

#[test]
fn labels_and_symbols() {
    // `key:` is one LABEL token, colon included. `:sym` is a SYMBOL_BEGIN that is the colon
    // alone — the second pass paints the whole PM_SYMBOL_NODE, quotes and all.
    let src = "f(key: 1, :sym => 2)";
    assert_eq!(map(src), "80555504005555000040");
    assert_eq!(of(src, "key:"), [SYMBOL; 4]);
    assert_eq!(of(src, ":sym"), [SYMBOL; 4]);
    assert_eq!(map(":Plant"), "555555"); // and not `:` + a constant
    assert_eq!(map(":\"quoted sym\""), "5555555555555");
    assert_eq!(map("h = { a: 1, :b => 2 }"), "000000550400550000400");
}

#[test]
fn constants_variables_and_numbers() {
    assert_eq!(map("Rubevy::Garden.new"), "666666006666660888");
    assert_eq!(map("@@c = $g"), "77700077");
    assert_eq!(map("@i = 0.06"), "770004444");
    assert_eq!(of("@i = 0.06", "0.06"), [NUMBER; 4]);
}

#[test]
fn word_arrays_and_heredocs() {
    // `%i[c]`'s element is a symbol node, so it is 5 inside a literal that is otherwise 2;
    // the `+` between the two arrays is an operator call, so it is a method name
    assert_eq!(map("%w[a b] + %i[c]"), "222222208022252");
    // the heredoc's body and terminator are string; the newline that opens it is not
    assert_eq!(map("s = <<~TXT\n  hi\nTXT\n"), "00002222220222222222");
}

#[test]
fn a_japanese_comment_is_one_run_and_never_splits_a_character() {
    // the garden's scripts are commented in Japanese, and `listing()` slices the line with
    // `&str`: a run must never end inside a character
    let src = "# 甲虫は歩く\np 1\n";
    assert_eq!(map(src), "3333333333333333338040");
    let comment = "# 甲虫は歩く";
    assert_eq!(comment.len(), 17); // bytes, not characters
    let m = highlight(src.as_bytes());
    assert_eq!(&m[..comment.len()], &[COMMENT; 17]);
    let mut changes = 0;
    for i in 1..m.len() {
        if m[i] != m[i - 1] {
            assert!(src.is_char_boundary(i), "a run ends inside a character at byte {i}");
            changes += 1;
        }
    }
    assert!(changes > 0);
}

#[test]
fn a_source_that_does_not_parse_still_gets_a_map() {
    // an editor's text is broken most of the time it is looked at. An unclosed parameter list
    // still has its DefNode, so the name is still painted, and the lexer has painted the rest.
    let src = "def foo(\n  @a = :x\n";
    assert_eq!(map(src), "1110888000077000550");
    assert_eq!(of(src, "foo"), [METHOD; 3]);
    assert_eq!(of(src, "@a"), [VARIABLE; 2]);
    assert_eq!(of(src, ":x"), [SYMBOL; 2]);
    // three `end`s with nothing to end, and a string that is never closed
    assert_eq!(map("end end end \"unterminated"), "1110111011102222222222222");
    // half-typed, which is what a keystroke looks like
    assert_eq!(map("me.ea"), "00088");
    assert_eq!(map("def fo"), "111088");
    // where the parser gives up on the body, the name it did parse is still right and the
    // call it never built is simply not painted
    assert_eq!(map("def foo(a,\n  b.c"), "1110888000000000");
}

#[test]
fn the_map_is_always_as_long_as_the_source() {
    assert!(highlight(b"").is_empty());
    for src in ["", "\n\n\n", "# only a comment", "あ", "x", " "] {
        assert_eq!(map(src).len(), src.len());
    }
}

/// No maximum source size is imposed; `csrc/shim.c` says why. The largest single file the
/// editor holds is the garden's `world.rb`, 16249 bytes on 2026-09-18.
#[test]
fn a_file_sized_source() {
    let src: String = std::iter::repeat_n("def step\n  @n += 1 # 進む\nend\n\n", 500).collect();
    assert!(src.len() > 16 * 1024, "{} bytes", src.len());
    let m = highlight(src.as_bytes());
    assert_eq!(m.len(), src.len());
    assert_eq!(&m[..3], &[KEYWORD; 3]);
    assert!(m.contains(&COMMENT));
    assert!(m.contains(&METHOD));
}

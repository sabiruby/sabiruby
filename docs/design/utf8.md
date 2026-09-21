# UTF-8 strings (the feature `utf8`)

SabiRuby reads a String as a sequence of **characters** by default, as the reference does when
it is built with `MRB_UTF8_STRING`, and as a sequence of **bytes** without the Cargo feature
`utf8`, which is the build the reference ships. One crate serves both: the feature is chosen by
whoever builds (`docs/plans/utf8-plan.md`, option B, decided by the author on 2026-09-12).

```toml
sabiruby = "0.6"                                              # characters (default)
sabiruby = { version = "0.6", default-features = false, features = ["std"] }   # bytes
```

`cargo build -p sabiruby-cli --no-default-features` builds the command the byte way;
`__ENCODING__` says which build is running (`"UTF-8"` or `"ASCII-8BIT"`), which is what the
reference's own tests ask.

## What counts characters and what counts bytes

`length`/`size`, `[]`/`slice`, `[]=`, `index`/`rindex`, `chars`, `each_char`, `reverse`,
`split`, `chop`, `chomp`, `succ`, `center`/`ljust`/`rjust`, `insert`'s padding, `ord`, `chr`,
`codepoints`, `sub`/`gsub` and the case methods count and cut **characters**.
`bytesize`, `bytes`, `byteslice`, `bytesplice`, `byteindex`, `byterindex`, `getbyte`,
`setbyte` count **bytes** in both builds, as they do in the reference; so do the width and the
precision of `sprintf` (`"%5s" % "あ"` pads to five bytes there and here).

A byte offset handed to a byte search has to name a position the string has:
`"aあb".byteindex("b", 2)` is an IndexError, "offset 2 does not land on character boundary"
(`mrb_str_check_byte_pos`). A needle whose bytes spell no character is found nowhere —
`index`, `rindex`, `byteindex`, `byterindex`, `[]`, `include?`, `end_with?`, `partition`,
`rpartition`, `chomp`, `slice!`, `delete_prefix`, `delete_suffix` all answer as if it were not
there (`str_index_str`), which is CRuby's answer too.

A run of bytes that spells no character is one position per byte, which is what `length`
counts over (`mrb_utf8len` rejects an overlong sequence, a UTF-16 surrogate and anything above
U+10FFFF, so `"\xED\xA0\x80".length` is 3). Reading such a run as a character is refused where
a character is what is asked for: `ord` and `codepoints` raise ArgumentError
("invalid UTF-8 byte sequence"), the case methods raise ArgumentError ("input string invalid"),
and `scrub` replaces each maximal subpart with U+FFFD (Unicode 3.9, so `"\xE0\x80\xAF".scrub`
is three replacements and `"\xE3\x81".scrub` one).

## Byte-read strings (`String#b`)

`String#b` answers a copy that has one position per byte whatever its bytes are
(`MRB_STR_ENCODING_BINARY`): `"あ".b.length` is 3, `"aあb".b.rindex("\x81".b)` is 2, and
`"\u{1F600}".b.chop` takes one byte off. Every string built out of such a string is read the
same way — a slice, a copy, `upcase`, `center`, `split`, the pieces of `partition`, the answer
of `sub` — and bytes that were read as bytes and go above ASCII hand their reading to the string
they are appended into (`str_cat_enc_check`), so `"あ" << "い".b` is byte-read from then on.
`+` decides for its own answer instead (`mrb_str_plus`): two byte-read operands stay that way,
and a byte-read operand carrying a byte above ASCII wins, while one of nothing but ASCII yields.

Only `String#b` makes such a string: `force_encoding` and `String#encoding` come with
mruby-encoding, which is not ported (and is absent from the reference build too).

## Unicode data

* **Case**: `upcase`/`downcase`/`capitalize`/`swapcase` and `casecmp?` use Rust's
  `char::to_uppercase`/`to_lowercase`, which is the Unicode mapping the reference compiles in
  unless it is narrowed by `MRB_USE_ASCII_CTYPE` (`"ß".upcase` is `"SS"`, `"ǳ".capitalize` is
  `"ǲ"`, `"ﬁ".capitalize` is `"Fi"`). Folding for `casecmp?` is the upper case put through the
  lower case, so `"ß".casecmp?("SS")` is true. A string of nothing but ASCII and a byte-read one
  convert their bytes where they stand, as `mrb_str_case_convert_unicode` does.
* **`String#succ`**: which code points above ASCII are letters and which are digits is
  `src/builtins/str_alnum.rs`, ported from the reference's generated `str_alnum.h` (the derived
  property Alphabetic and the decimal digits of the Unicode character database). A letter steps
  to the next letter of its run and wraps to the run's first one with a carry, so `"ת".succ` is
  `"אא"` and `"ｚ".succ` is `"ａａ"`.

## Deviations kept

* **The reference's byte-for-character slips are not copied.** Five methods of
  mruby-string-ext hand a *byte* offset to `mrb_str_substr`, which counts *characters* in a
  UTF-8 build, so they cut at the wrong place: `delete_prefix` (`"あい".delete_prefix("あ")` is
  nil there), `delete_suffix` (the whole string back), `strip`, `lstrip` and `rstrip`
  (`"  あ  ".strip` is `"あ  "`). SabiRuby cuts where the offset was measured, which is CRuby's
  answer; `tests/custom/utf8_reference_bugs.rb` holds the cases with CRuby 3.2's output.
  mruby's own mrblib `sub`/`gsub` mix the two units the same way — the reference never runs
  them, because mruby-regexp replaces both with C ones, and SabiRuby registers native ones
  after mrblib for the same reason (`string::post_mrblib`).
* **`codepoints` of a byte-read string** answers the byte (0 to 255). The reference's byte
  build pushes `(mrb_int)*p` over a signed `char`, so `"あ".codepoints` is `[-29, -127, -126]`
  where `char` is signed (x86) and `[227, 129, 130]` where it is not (ARM); that is the
  platform's answer rather than a decision.
* **Swapping case above ASCII** uses the rule "a character with a lower case swaps down, one
  without swaps up", plus the four Latin digraphs in title case (`"ǅ"` to `"dŽ"`). The
  reference's swap table also holds 27 Greek forms with the iota subscript, which are left to
  the rule here.
* **`%c` with a String** asks for one byte, as the reference does in both builds
  (`RSTRING_LEN(tmp) != 1`), so `"%c" % "い"` is an ArgumentError.

## How it is checked

* Two reference images: `kishima/mruby:4.1.0-rc` (bytes, the reference's own build) and
  `kishima/mruby:4.1.0-rc-utf8` (the same tree with `MRB_UTF8_STRING`), both built by
  `../ref/mruby_containers/build_image.sh`.
* mruby's own test suite runs in both builds and each has its own floor:
  `tools/mrbtest.sh [--update]` writes `docs/verification/mrbtest.md` and `tests/mrbtest/baseline.txt`,
  `tools/mrbtest.sh --bytes [--update]` writes `docs/verification/mrbtest-bytes.md` and
  `tests/mrbtest/baseline-bytes.txt`. The same test bytecode serves both: which assertions run
  is decided at run time by `__ENCODING__` and by `"Ä".downcase == "ä"`.
* `tests/fixtures/utf8.rb` runs Japanese text through the string methods and is compared with
  the image of the build's own reading (`utf8.out` / `utf8-bytes.out`).
* `tests/custom/utf8_reference_bugs.rb` is marked `# utf8-only:` and is left out of a
  byte-string build.

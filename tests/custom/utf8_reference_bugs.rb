# utf8-only: strings as characters (the feature `utf8`)
# Character-indexed strings where mruby 4.1.0-rc2 (and -rc) built with MRB_UTF8_STRING answers
# something it does not answer for an ASCII string. Each one hands a BYTE offset to
# `mrb_str_substr`, which counts CHARACTERS in that build, so the answer is cut at the
# wrong place — a slip, not a decision, so SabiRuby cuts where the offset was measured
# (docs/design/utf8.md, "Deviations kept"):
#   delete_prefix   `str_del_prefix` calls mrb_str_substr(self, plen, slen-plen) with the
#                   prefix's byte length; for "あい".delete_prefix("あ") that is substr(3, 3)
#                   over a two-character string, which is out of range: nil.
#   delete_suffix   `str_del_suffix` calls mrb_str_substr(self, 0, slen-plen), which is
#                   three characters of a two-character string: the whole string back.
#   strip family    `str_strip`/`str_lstrip`/`str_rstrip` measure the run of whitespace in
#                   bytes and cut that many characters, so a multi-byte character before
#                   the trailing spaces leaves them in place.
# expected-from: CRuby 3.2 (run: ruby 3.2, the values below are its output)
p "あい".delete_prefix("あ")
p "あい".delete_suffix("い")
p "あい".delete_prefix("\xE3")
p "  あ  ".strip
p "  あ  ".rstrip
p "  あ  ".lstrip
p "あ\t\n".strip

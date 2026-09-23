#!/bin/bash
# What classes, modules and methods SabiRuby has, next to the reference mruby 4.1.0-rc2,
# written to docs/verification/coverage.md.
#   tools/coverage.sh            # build (release), run both, write the document
#   MRUBY_BIN=/path/to/mruby tools/coverage.sh   # use that binary instead of Docker
# Both sides run the same script, tools/coverage.rb; the reference runs in the same image
# tools/mrbtest.sh compiles with (kishima/mruby:4.1.0-rc2), whose `mruby` is built from
# default.gembox plus the POSIX gems. This is the "what is there" answer, next to
# tools/mrbtest.sh's "does it behave the same".
set -eu
cd "$(dirname "$0")/.."
IMG=kishima/mruby:4.1.0-rc2
OUT=docs/verification/coverage.md
WORK=target/coverage
mkdir -p $WORK

cargo build --release -q -p sabiruby-cli
./target/release/sabiruby run tools/coverage.rb > $WORK/sabiruby.txt
cp tools/coverage.rb $WORK/coverage.rb
if [ -n "${MRUBY_BIN:-}" ]; then
  "$MRUBY_BIN" $WORK/coverage.rb > $WORK/reference.txt
else
  docker run --rm -v "$PWD/$WORK:/w" $IMG mruby /w/coverage.rb > $WORK/reference.txt
fi

# The two lists are tagged with the side they came from and merged by one awk program.
# awk only creates a list file when it has a line for it, so a category that is empty this
# time would otherwise keep the last run's file (and its lines).
for f in only_reference only_sabiruby moved_reference moved_sabiruby \
         modfunc_reference modfunc_sabiruby visibility; do : > $WORK/$f.txt; done

{ sed 's/^/S\t/' $WORK/sabiruby.txt; sed 's/^/R\t/' $WORK/reference.txt; } |
awk -v work="$WORK" '
BEGIN {
  FS = "\t"; OFS = "\t"
  # Which gem a class comes from. Not mechanical: mruby tells a script nothing about where a
  # method was defined (no Method#source_location, and Method#owner answers the class, not the
  # gem), so this is the one hand-kept table in the generation, read off the gem list in
  # docs/design/gems.md. A class missing from it is core (or listed with its own gem below).
  g["Fiber"]="mruby-fiber"; g["FiberError"]="mruby-fiber"
  g["Enumerator"]="mruby-enumerator"; g["Enumerator::Generator"]="mruby-enumerator"
  g["Enumerator::Yielder"]="mruby-enumerator"
  g["Enumerator::Lazy"]="mruby-enum-lazy"; g["Enumerator::Chain"]="mruby-enum-chain"
  g["ObjectSpace"]="mruby-objectspace"
  g["Math"]="mruby-math"; g["Math::DomainError"]="mruby-math"
  g["Random"]="mruby-random"; g["Struct"]="mruby-struct"; g["Data"]="mruby-data"
  g["Set"]="mruby-set"; g["Time"]="mruby-time"
  g["Rational"]="mruby-rational"; g["Complex"]="mruby-complex"; g["CMath"]="mruby-cmath"
  g["Regexp"]="mruby-regexp"; g["RegexpError"]="mruby-regexp"; g["MatchData"]="mruby-regexp"
  g["Binding"]="mruby-binding"
  g["Method"]="mruby-method"; g["UnboundMethod"]="mruby-method"
  g["UncaughtThrowError"]="mruby-catch"
  g["Task"]="mruby-task"; g["Task::Error"]="mruby-task"; g["Task::Overrun"]="mruby-task"
  g["Task::Queue"]="mruby-task"
  g["LoadError"]="sabiruby (require/load)"
  # POSIX gems: deliberately not ported (the VM is no_std; docs/design/gems.md,
  # "Remaining gems"). Their classes are collapsed into one row per gem below and their
  # methods are counted apart from the difference lists.
  g["IO"]="mruby-io"; g["IOError"]="mruby-io"; g["EOFError"]="mruby-io"; g["File"]="mruby-io"
  g["File::Constants"]="mruby-io"; g["FileTest"]="mruby-io"
  g["Dir"]="mruby-dir"
  g["Errno"]="mruby-errno"; g["SystemCallError"]="mruby-errno"
  g["Socket"]="mruby-socket"; g["Socket::Constants"]="mruby-socket"
  g["Socket::Option"]="mruby-socket"; g["BasicSocket"]="mruby-socket"
  g["IPSocket"]="mruby-socket"; g["TCPSocket"]="mruby-socket"; g["TCPServer"]="mruby-socket"
  g["UDPSocket"]="mruby-socket"; g["UNIXSocket"]="mruby-socket"; g["UNIXServer"]="mruby-socket"
  g["Addrinfo"]="mruby-socket"; g["SocketError"]="mruby-socket"
  g["Process"]="mruby-process"; g["Process::Status"]="mruby-process"
  g["Process::Tms"]="mruby-process"
  g["Signal"]="mruby-signal"
  skip["mruby-io"]=1; skip["mruby-dir"]=1; skip["mruby-errno"]=1; skip["mruby-socket"]=1
  skip["mruby-process"]=1; skip["mruby-signal"]=1
}
function gem_of(c,   p) {
  if (c in g) return g[c]
  p = c; sub(/::.*/, "", p)
  if (p != c && (p in g)) return g[p]          # Errno::ENOENT, Socket::Option, …
  return "core"
}
function owner_of(m) { match(m, /[#.]/); return substr(m, 1, RSTART - 1) }
# The same name on the same class with the other separator. mruby defines a module
# function as a public singleton method *and* a private instance method, and
# `instance_methods` answers only the public and protected ones, so one side often has
# `Kernel.sprintf` where the other has `Kernel#sprintf`.
function other(m) { return owner_of(m) (sep_of(m) == "#" ? "." : "#") name_of(m) }
function sep_of(m)   { match(m, /[#.]/); return substr(m, RSTART, 1) }
function name_of(m)  { match(m, /[#.]/); return substr(m, RSTART + 1) }
# Where a side answers <cls><sep><meth> from, "" if it does not answer it at all.
# An instance method is looked for along the ancestors; a class method along the
# superclass chain (the classes of the ancestors, in order) and then on the ancestors of
# Class/Module, which is where Module#name and friends come from.
function lives(side, cls, sep, meth,   i, n, a, chain) {
  if (!((side, cls) in kind)) return ""
  n = split(anc[side, cls], a, ",")
  for (i = 1; i <= n; i++) {
    if (sep == "#") { if ((side, a[i] "#" meth) in has) return a[i] "#" meth }
    else if (kind[side, a[i]] == "class" || i == 1) {
      if ((side, a[i] "." meth) in has) return a[i] "." meth
    }
  }
  if (sep == ".") {
    chain = (kind[side, cls] == "class") ? "Class" : "Module"
    n = split(anc[side, chain], a, ",")
    for (i = 1; i <= n; i++) if ((side, a[i] "#" meth) in has) return a[i] "#" meth
  }
  return ""
}
$2 == "#engine" { engine[$1] = $3 "/" $4; desc[$1] = $6 }
$2 == "#note"   { note[$1] = $3 }
$2 == "class" {
  kind[$1, $3] = $4; anc[$1, $3] = $6; allcls[$3] = 1
  if ($1 == "S") ncls_s++; else ncls_r++
}
$2 == "method" {
  has[$1, $3] = 1; vis[$1, $3] = $4; own[$1, owner_of($3)]++
  if ($1 == "S") nm_s++; else nm_r++
}
END {
  cls_tbl  = work "/tbl_classes.txt"
  only_r   = work "/only_reference.txt"
  only_s   = work "/only_sabiruby.txt"
  moved_r  = work "/moved_reference.txt"
  moved_s  = work "/moved_sabiruby.txt"
  mfun_r   = work "/modfunc_reference.txt"
  mfun_s   = work "/modfunc_sabiruby.txt"
  gem_tbl  = work "/tbl_gems.txt"
  visdiff  = work "/visibility.txt"
  head     = work "/head.txt"

  for (k in has) {
    split(k, kk, SUBSEP); side = kk[1]; m = kk[2]
    c = owner_of(m); gm = gem_of(c)
    if (side == "S" && !(("R", m) in has)) {
      if (skip[gm]) { skipped_s++; continue }
      o = other(m)
      if (("R", o) in has) { print m "\t" o > mfun_s; n_mfun_s++ }
      else {
        w = lives("R", c, sep_of(m), name_of(m))
        if (w == "") { print m > only_s; n_only_s++ }
        else         { print m "\t" w > moved_s; n_moved_s++ }
      }
    }
    if (side == "R" && !(("S", m) in has)) {
      if (skip[gm]) { skipped_r++; continue }
      o = other(m)
      if (("S", o) in has) { print m "\t" o > mfun_r; n_mfun_r++ }
      else {
        w = lives("S", c, sep_of(m), name_of(m))
        if (w == "") { print m > only_r; n_only_r++ }
        else         { print m "\t" w > moved_r; n_moved_r++ }
      }
    }
    if (side == "S" && (("R", m) in has)) {
      nm_both++
      if (vis["S", m] != vis["R", m] && !skip[gm])
        { print m "\t" vis["S", m] "\t" vis["R", m] > visdiff; n_vis++ }
    }
  }

  for (c in allcls) {
    gm = gem_of(c)
    s = own["S", c] + 0; r = own["R", c] + 0
    if (skip[gm]) {                       # collapsed to one row per gem
      gcls[gm]++; gs[gm] += s; gr[gm] += r
      continue
    }
    if (("S", c) in kind) ncls_both += (("R", c) in kind) ? 1 : 0
    ss = (("S", c) in kind) ? s : "–"
    rr = (("R", c) in kind) ? r : "–"
    printf "| `%s` | %s | %s | %s |\n", c, gm, ss, rr > cls_tbl
  }
  for (gm in gcls)
    printf "| %d class%s | %s | %s | %d |\n", gcls[gm], (gcls[gm] == 1 ? "" : "es"), gm,
           (gs[gm] ? gs[gm] : "–"), gr[gm] > gem_tbl

  print "sabiruby-classes\t" ncls_s  > head
  print "reference-classes\t" ncls_r > head
  print "both-classes\t" ncls_both   > head
  print "sabiruby-methods\t" nm_s    > head
  print "reference-methods\t" nm_r   > head
  print "both-methods\t" nm_both + 0 > head
  print "only-sabiruby\t" n_only_s + 0   > head
  print "only-reference\t" n_only_r + 0  > head
  print "moved-sabiruby\t" n_moved_s + 0 > head
  print "moved-reference\t" n_moved_r + 0 > head
  print "modfunc-sabiruby\t" n_mfun_s + 0  > head
  print "modfunc-reference\t" n_mfun_r + 0 > head
  print "visibility\t" n_vis + 0 > head
  print "skipped-sabiruby\t" skipped_s + 0  > head
  print "skipped-reference\t" skipped_r + 0 > head
  print "engine-sabiruby\t" engine["S"] "\t" desc["S"] > head
  print "engine-reference\t" engine["R"] "\t" desc["R"] > head
  print "note-sabiruby\t" note["S"]  > head
  print "note-reference\t" note["R"] > head
}
'
v() { grep -m1 "^$1	" $WORK/head.txt | cut -f2-; }
n() { grep -m1 "^$1	" $WORK/head.txt | cut -f2; }
# A name as a Markdown code span. One of them is Kernel's backtick method, which needs
# the doubled fence (`` Kernel.` ``).
CODE='function code(x) { return index(x, "`") ? "`` " x " ``" : "`" x "`" }'
# A list of `Class#method` lines as Markdown bullets, "(none)" when empty.
bullets() { if [ -s "$1" ]; then sort "$1" | awk "$CODE"'{ printf "* %s\n", code($0) }'; else echo "(none)"; fi; }
# The same for the two-column lists: what one side has → where the other side has it.
arrows()  { if [ -s "$1" ]; then sort "$1" | awk -F'\t' "$CODE"'{ printf "* %s → %s\n", code($1), code($2) }'; else echo "(none)"; fi; }
{
  echo "# What SabiRuby has"
  echo
  echo "Every class, module and method SabiRuby defines, next to the reference mruby 4.1.0-rc2."
  echo "\`tools/mrbtest.sh\` answers *does it behave the same*; this answers *what is there at all*."
  echo
  echo "Generated by \`tools/coverage.sh\` on $(date +%Y-%m-%d). Do not edit."
  echo "Both sides run the same script, \`tools/coverage.rb\`, which walks the constants from"
  echo "\`Object\` and asks each class and module for \`instance_methods(false)\` and its singleton"
  echo "class's \`instance_methods(false)\`, then sweeps \`ObjectSpace\` for anything the constants"
  echo "did not reach. A method is listed **once, under the module that defines it**; \`ancestors\`"
  echo "says who inherits it. Private methods are counted too (\`private_instance_methods(false)\`):"
  echo "mruby writes a module function as a public singleton method and a private instance method of"
  echo "the same name, and keeps a good part of \`Module\` and \`Kernel\` private, so leaving them out"
  echo "would turn half the difference between the two VMs into an artefact."
  echo "The reference is the \`mruby\` of the image \`$IMG\`"
  echo "(\`default.gembox\` plus the POSIX gems), the same image \`tools/mrbtest.sh\` compiles with."
  echo
  echo "* SabiRuby: \`RUBY_ENGINE\`/\`RUBY_ENGINE_VERSION\` = $(v engine-sabiruby | cut -f1), \`MRUBY_DESCRIPTION\` = $(v engine-sabiruby | cut -f2)"
  echo "* reference: \`RUBY_ENGINE\`/\`RUBY_ENGINE_VERSION\` = $(v engine-reference | cut -f1), \`MRUBY_DESCRIPTION\` = $(v engine-reference | cut -f2)"
  echo
  echo "(Both answer \`mruby\`: what SabiRuby should answer is item 5 of"
  echo "[\`plans/from-mrubyedge-plan.md\`](../plans/from-mrubyedge-plan.md).)"
  echo
  echo "## Summary"
  echo
  echo "| | classes and modules | methods |"
  echo "|---|---:|---:|"
  echo "| SabiRuby has | $(n sabiruby-classes) | $(n sabiruby-methods) |"
  echo "| the reference has | $(n reference-classes) | $(n reference-methods) |"
  echo "| both | $(n both-classes) | $(n both-methods) |"
  echo "| only SabiRuby | $(( $(n sabiruby-classes) - $(n both-classes) )) | $(( $(n sabiruby-methods) - $(n both-methods) )) |"
  echo "| only the reference | $(( $(n reference-classes) - $(n both-classes) )) | $(( $(n reference-methods) - $(n both-methods) )) |"
  echo
  echo "A difference in that table is not the same as a method that does not answer. Of the"
  echo "$(( $(n reference-methods) - $(n both-methods) )) the reference has and SabiRuby has not:"
  echo
  echo "* **$(n skipped-reference)** belong to the POSIX gems, which are not planned (\`design/gems.md\`, \"Remaining gems\")."
  echo "* **$(n modfunc-reference)** are the other half of a module-function pair: the same name on the same"
  echo "  module, singleton on one side and instance on the other."
  echo "* **$(n moved-reference)** answer here from another ancestor — the same method, a different owner."
  echo "* **$(n only-reference)** do not answer at all. That is the first list at the end of this file."
  echo
  echo "And of the $(( $(n sabiruby-methods) - $(n both-methods) )) SabiRuby has and the reference has not:"
  echo "**$(n skipped-sabiruby)** are in classes of a not-planned gem, **$(n modfunc-sabiruby)** are the other half of a"
  echo "module-function pair, **$(n moved-sabiruby)** are the same method on another owner, and"
  echo "**$(n only-sabiruby)** are additions."
  echo
  echo "The gem a method comes from is not asked per method: mruby tells a script nothing about"
  echo "where a method was defined (there is no \`Method#source_location\`, and \`Method#owner\`"
  echo "answers the class), so the gem column below is per class and hand-kept in"
  echo "\`tools/coverage.sh\`. A gem that adds methods to a core class is invisible in it:"
  echo "\`Kernel.gets\` (mruby-io) and \`Kernel.printf\` (mruby-io) count as core."
  echo
  echo "What the constant walk could not reach (unnamed classes are singleton classes, which"
  echo "carry the class methods already listed as \`Class.method\`):"
  echo
  echo "* SabiRuby: $(v note-sabiruby)"
  echo "* reference: $(v note-reference)"
  echo
  echo "## Per class"
  echo
  echo "Methods each class or module defines itself. \`–\` means the class is not there at all."
  echo "The classes of the not-planned POSIX gems are collapsed into one row per gem, at the end."
  echo
  echo "| class or module | gem | SabiRuby | reference |"
  echo "|---|---|---:|---:|"
  sort -f $WORK/tbl_classes.txt
  sort -f -t'|' -k3,3 $WORK/tbl_gems.txt
  echo
  echo "## In the reference, not in SabiRuby"
  echo
  echo "$(n only-reference) methods that do not answer here (the POSIX gems left out)."
  echo
  bullets $WORK/only_reference.txt
  echo
  echo "### The same method, another owner"
  echo
  echo "$(n moved-reference) more are defined on a different class or module here, so a program"
  echo "calling them works; only \`Method#owner\` and \`instance_methods(false)\` differ."
  echo
  arrows $WORK/moved_reference.txt
  echo
  echo "### The other half of a module-function pair"
  echo
  echo "$(n modfunc-reference) more are a module function the reference exposes as a singleton method and"
  echo "SabiRuby as a public instance method, or the other way round. Both are callable."
  echo
  arrows $WORK/modfunc_reference.txt
  echo
  echo "## In SabiRuby, not in the reference"
  echo
  echo "$(n only-sabiruby) methods the reference does not have."
  echo
  bullets $WORK/only_sabiruby.txt
  echo
  echo "### The same method, another owner"
  echo
  echo "$(n moved-sabiruby) more are the reference's methods moved to another owner."
  echo
  arrows $WORK/moved_sabiruby.txt
  echo
  echo "### The other half of a module-function pair"
  echo
  echo "$(n modfunc-sabiruby) more are the other half of a module-function pair: the same name on the"
  echo "same module, instance on this side and singleton on the reference's."
  echo
  arrows $WORK/modfunc_sabiruby.txt
  echo
  echo "## On both sides, with a different visibility"
  echo
  echo "$(n visibility) methods both VMs define on the same class but under a different visibility."
  echo "Protected counts as public here, so every line is a private method on one side and a"
  echo "public one on the other."
  echo
  if [ -s $WORK/visibility.txt ]; then
    sort $WORK/visibility.txt | awk -F'\t' "$CODE"'{ printf "* %s: %s here, %s in the reference\n", code($1), $2, $3 }'
  else
    echo "(none)"
  fi
} > $OUT
echo "wrote $OUT ($(wc -l < $OUT) lines)"

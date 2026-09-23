/* The whole C side of sabiruby-compiler: one call from source to RITE binary.
   It does what the reference mrbc (mrbgems/mruby-bin-mrbc/tools/mrbc/mrbc.c)
   does, reading from memory instead of files, so that Rust never touches the
   compiler's structs (mrc_ccontext has bit fields). */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "mrc_irep.h"
#include "mrc_ccontext.h"
#include "mrc_dump.h"
#include "mrc_compile.h"
#include "mrc_diagnostic.h"

/* Flags (keep in sync with src/ffi.rs). */
#define SHIM_DEBUG_INFO  1u /* mrbc -g */
#define SHIM_REMOVE_LV   2u /* mrbc --remove-lv */
#define SHIM_NO_EXT_OPS  4u /* mrbc --no-ext-ops */
#define SHIM_NO_OPTIMIZE 8u /* mrbc --no-optimize */

/* Results. */
#define SHIM_OK            0
#define SHIM_COMPILE_ERROR 1 /* diagnostics in *diag */
#define SHIM_DUMP_ERROR    2
#define SHIM_NO_MEMORY     3

/* The reference mrbc.c also defines dummy mrb_intern / mrb_sym_name. Every call
   to them in mruby-compiler is under MRC_TARGET_MRUBY, so the standalone build does
   not reference them (checked with nm), and defining them here would clash with a
   real mruby linked into the same program. */

/* Growable text buffer for the diagnostics. */
typedef struct { char *p; size_t len, cap; int oom; } sbuf;

static void
sbuf_put(sbuf *b, const char *s, size_t n)
{
  if (b->oom) return;
  if (b->len + n + 1 > b->cap) {
    size_t cap = b->cap ? b->cap * 2 : 256;
    while (cap < b->len + n + 1) cap *= 2;
    char *q = (char *)realloc(b->p, cap);
    if (!q) { b->oom = 1; return; }
    b->p = q; b->cap = cap;
  }
  memcpy(b->p + b->len, s, n);
  b->len += n;
  b->p[b->len] = '\0';
}

static void
sbuf_str(sbuf *b, const char *s)
{
  sbuf_put(b, s ? s : "", s ? strlen(s) : 0);
}

static void
sbuf_u32(sbuf *b, uint32_t v)
{
  char tmp[12];
  int i = (int)sizeof(tmp);
  tmp[--i] = '\0';
  do { tmp[--i] = (char)('0' + v % 10); v /= 10; } while (v && i > 0);
  sbuf_str(b, tmp + i);
}

/* Diagnostics as records separated by 0x1e, fields by 0x1f:
   code, line, column, filename, message (the message may contain tabs and newlines). */
static char *
collect_diagnostics(mrc_ccontext *c)
{
  sbuf b = { NULL, 0, 0, 0 };
  for (mrc_diagnostic_list *d = c->diagnostic_list; d; d = d->next) {
    const char *filename = d->filename ? d->filename
                         : (c->filename_table ? c->filename_table[0].filename : "-");
    sbuf_u32(&b, (uint32_t)d->code); sbuf_put(&b, "\x1f", 1);
    sbuf_u32(&b, d->line);           sbuf_put(&b, "\x1f", 1);
    sbuf_u32(&b, d->column);         sbuf_put(&b, "\x1f", 1);
    sbuf_str(&b, filename);          sbuf_put(&b, "\x1f", 1);
    sbuf_str(&b, d->message);        sbuf_put(&b, "\x1e", 1);
  }
  if (b.oom) { free(b.p); return NULL; }
  return b.p;
}

int
sabiruby_mrc_compile(const uint8_t *src, size_t len, const char *filename, unsigned flags,
                     uint8_t **out, size_t *out_len, char **diag)
{
  *out = NULL; *out_len = 0; *diag = NULL;
  mrc_ccontext *c = mrc_ccontext_new(NULL);
  if (!c) return SHIM_NO_MEMORY;
  if (filename && !mrc_ccontext_filename(c, filename)) { mrc_ccontext_free(c); return SHIM_NO_MEMORY; }
  c->no_exec = 1; /* as mrbc's load_file */
  c->no_ext_ops = (flags & SHIM_NO_EXT_OPS) ? 1 : 0;
  c->no_optimize = (flags & SHIM_NO_OPTIMIZE) ? 1 : 0;

  /* NUL-terminated copy, like the buffer mrbc reads a file into; the parser
     keeps pointing into it until the irep is freed */
  uint8_t *buf = (uint8_t *)malloc(len + 1);
  if (!buf) { mrc_ccontext_free(c); return SHIM_NO_MEMORY; }
  if (len) memcpy(buf, src, len);
  buf[len] = '\0';
  const uint8_t *source = buf;

  int result;
  mrc_irep *irep = mrc_load_string_cxt(c, &source, len);
  *diag = collect_diagnostics(c);
  if (!irep) {
    result = SHIM_COMPILE_ERROR;
  }
  else {
    if (flags & SHIM_REMOVE_LV) mrc_irep_remove_lv(c, irep);
    uint8_t *bin = NULL;
    size_t size = 0;
    int n = mrc_dump_irep(c, irep, (flags & SHIM_DEBUG_INFO) ? MRC_DUMP_DEBUG_INFO : 0, &bin, &size);
    if (n == MRC_DUMP_OK) {
      *out = bin; *out_len = size;
      result = SHIM_OK;
    }
    else {
      free(bin);
      result = SHIM_DUMP_ERROR;
    }
    mrc_irep_free(c, irep);
  }
  mrc_ccontext_free(c);
  free(buf);
  return result;
}

/* Compiles an eval string: like sabiruby_mrc_compile, plus the enclosing local variable
   names (so that the string can read and write the caller's variables) and the line the
   string starts at. `scopes` is a flat blob, little-endian:

     scopes := nscopes * scope
     scope  := u32 count, count * (u16 length, length bytes)

   scope 0 is the caller, each next one is further out; a zero-length name is a hole in the
   caller's local variable table (an unnamed parameter), which keeps the positions right.
   `filename` shows in diagnostics and in the debug info. */
int
sabiruby_mrc_compile_eval(const uint8_t *src, size_t len, const char *filename, uint32_t line,
                          unsigned flags, const uint8_t *scopes, size_t scopes_len, uint32_t nscopes,
                          uint8_t **out, size_t *out_len, char **diag)
{
  *out = NULL; *out_len = 0; *diag = NULL;

  /* unpack the blob into the compiler's view of it */
  struct sabiruby_eval_scope *tab = NULL;
  const char **names = NULL;
  size_t *lengths = NULL;
  size_t total = 0;
  const uint8_t *p = scopes, *end = scopes + scopes_len;
  if (nscopes > 0) {
    /* first pass: how many names */
    for (uint32_t i = 0; i < nscopes; i++) {
      if (p + 4 > end) return SHIM_NO_MEMORY;
      uint32_t count = (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
      p += 4;
      for (uint32_t j = 0; j < count; j++) {
        if (p + 2 > end) return SHIM_NO_MEMORY;
        size_t l = (size_t)p[0] | ((size_t)p[1] << 8);
        p += 2 + l;
        if (p > end) return SHIM_NO_MEMORY;
      }
      total += count;
    }
    tab = (struct sabiruby_eval_scope *)calloc(nscopes, sizeof(*tab));
    names = (const char **)calloc(total ? total : 1, sizeof(*names));
    lengths = (size_t *)calloc(total ? total : 1, sizeof(*lengths));
    if (!tab || !names || !lengths) { free(tab); free(names); free(lengths); return SHIM_NO_MEMORY; }
    p = scopes;
    size_t at = 0;
    for (uint32_t i = 0; i < nscopes; i++) {
      uint32_t count = (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
      p += 4;
      tab[i].count = count;
      tab[i].names = names + at;
      tab[i].lengths = lengths + at;
      for (uint32_t j = 0; j < count; j++) {
        size_t l = (size_t)p[0] | ((size_t)p[1] << 8);
        p += 2;
        names[at] = (const char *)p;
        lengths[at] = l;
        at++;
        p += l;
      }
    }
  }
  struct sabiruby_eval_scopes all = { nscopes, tab };

  mrc_ccontext *c = mrc_ccontext_new(NULL);
  if (!c) { free(tab); free(names); free(lengths); return SHIM_NO_MEMORY; }
  if (filename && !mrc_ccontext_filename(c, filename)) { mrc_ccontext_free(c); free(tab); free(names); free(lengths); return SHIM_NO_MEMORY; }
  c->no_exec = 1;
  c->no_optimize = 1; /* as the reference's eval (mruby-eval/src/eval.c) */
  c->lineno = (uint16_t)line;
  if (nscopes > 0) c->eval_scopes = &all;

  uint8_t *buf = (uint8_t *)malloc(len + 1);
  if (!buf) { mrc_ccontext_free(c); free(tab); free(names); free(lengths); return SHIM_NO_MEMORY; }
  if (len) memcpy(buf, src, len);
  buf[len] = '\0';
  const uint8_t *source = buf;

  int result;
  mrc_irep *irep = mrc_load_string_cxt(c, &source, len);
  *diag = collect_diagnostics(c);
  if (!irep) {
    result = SHIM_COMPILE_ERROR;
  }
  else {
    uint8_t *bin = NULL;
    size_t size = 0;
    int n = mrc_dump_irep(c, irep, (flags & SHIM_DEBUG_INFO) ? MRC_DUMP_DEBUG_INFO : 0, &bin, &size);
    if (n == MRC_DUMP_OK) { *out = bin; *out_len = size; result = SHIM_OK; }
    else { free(bin); result = SHIM_DUMP_ERROR; }
    mrc_irep_free(c, irep);
  }
  mrc_ccontext_free(c);
  free(buf);
  free(tab); free(names); free(lengths);
  return result;
}

#ifdef SABIRUBY_SHIM_AST
/* Prism's pretty-printed tree of the source (what a debug mrbc prints for --verbose, and what
   mruby's code generator walks). Parsed as mrc parses it: no options, the file name as the
   file path (it shows in SourceFileNode). Returns a malloc'ed NUL-terminated string, or NULL. */
char *
sabiruby_mrc_ast(const uint8_t *src, size_t len, const char *filename)
{
  pm_options_t options = { 0 };
  pm_options_line_set(&options, 1); /* the default when no options are given, as mrc parses */
  if (filename) pm_options_filepath_set(&options, filename);
  pm_parser_t parser;
  pm_parser_init(&parser, src, len, &options);
  pm_node_t *root = pm_parse(&parser);
  pm_buffer_t buf;
  char *out = NULL;
  if (pm_buffer_init(&buf)) {
    pm_prettyprint(&buf, &parser, root);
    out = (char *)malloc(pm_buffer_length(&buf) + 1);
    if (out) {
      memcpy(out, pm_buffer_value(&buf), pm_buffer_length(&buf));
      out[pm_buffer_length(&buf)] = '\0';
    }
    pm_buffer_free(&buf);
  }
  pm_node_destroy(&parser, root);
  pm_parser_free(&parser);
  pm_options_free(&options);
  return out;
}
#endif

/* ---------------------------------------------------------------------------
   Syntax highlighting: one category byte per source byte.

   Moved from family-mruby's picoruby-syntax-highlight
   (fmruby-core/lib/add/picoruby-syntax-highlight/src/syntax_highlight.c),
   without its mruby binding (no mrb_value, no mrb_malloc) and otherwise whole:
   the same nine categories, the same pm_lex_callback_t, and the same second
   pass over the tree. Prism is 1.9.0 on both sides, so no token name had to
   change.

   Two passes, in family-mruby's order:

     1. the lexer. One call per token, each token's start..end painted from its
        type. This is what keeps a source that does not parse readable: the
        lexer runs ahead of the parser and has already written every token it
        reached.
     2. the tree (pm_visit_node). What a token type cannot say: a method name is
        an IDENTIFIER like any other, and a symbol's name is a token of its own.
        The second pass overwrites the first, so `p 1`'s `p`, `x =~ y`'s `=~`,
        `def ==`'s `==` and the whole of `:Plant` end up right. The garden's
        scripts are mostly bare calls (`tell :all, "season", s`, `sleep 0.5`,
        `every 60 do`), which the lexer alone cannot see at all.

   Categories (keep in sync with src/lib.rs and the plan's table):
     0 default  1 keyword  2 string  3 comment  4 number
     5 symbol   6 constant 7 variable 8 method name

   There is no maximum source size. family-mruby caps at 32 KiB because it runs
   on an ESP32's fixed heap; here the map costs one byte per source byte next to
   a source the caller already holds, and the whole Ruby of the garden demo --
   prelude.rb 21433 + world_prelude.rb 17161 + world.rb 16249 + creatures/
   beetle.rb 5870 + rabbit.rb 3632 = 64345 bytes -- is already nearly twice that cap,
   while the largest single buffer the editor ever holds is world.rb's 16 KiB.
   A cap would be a number with no budget behind it. */

#define HIGHLIGHT_DEFAULT   0
#define HIGHLIGHT_KEYWORD   1
#define HIGHLIGHT_STRING    2
#define HIGHLIGHT_COMMENT   3
#define HIGHLIGHT_NUMBER    4
#define HIGHLIGHT_SYMBOL    5
#define HIGHLIGHT_CONSTANT  6
#define HIGHLIGHT_VARIABLE  7
#define HIGHLIGHT_METHOD    8

typedef struct {
  uint8_t       *map;
  size_t         size;
  const uint8_t *source;
} highlight_data_t;

static uint8_t
token_type_to_category(pm_token_type_t type)
{
  switch (type) {
  /* Keywords */
  case PM_TOKEN_KEYWORD_ALIAS:
  case PM_TOKEN_KEYWORD_AND:
  case PM_TOKEN_KEYWORD_BEGIN:
  case PM_TOKEN_KEYWORD_BEGIN_UPCASE:
  case PM_TOKEN_KEYWORD_BREAK:
  case PM_TOKEN_KEYWORD_CASE:
  case PM_TOKEN_KEYWORD_CLASS:
  case PM_TOKEN_KEYWORD_DEF:
  case PM_TOKEN_KEYWORD_DEFINED:
  case PM_TOKEN_KEYWORD_DO:
  case PM_TOKEN_KEYWORD_DO_LOOP:
  case PM_TOKEN_KEYWORD_ELSE:
  case PM_TOKEN_KEYWORD_ELSIF:
  case PM_TOKEN_KEYWORD_END:
  case PM_TOKEN_KEYWORD_END_UPCASE:
  case PM_TOKEN_KEYWORD_ENSURE:
  case PM_TOKEN_KEYWORD_FALSE:
  case PM_TOKEN_KEYWORD_FOR:
  case PM_TOKEN_KEYWORD_IF:
  case PM_TOKEN_KEYWORD_IF_MODIFIER:
  case PM_TOKEN_KEYWORD_IN:
  case PM_TOKEN_KEYWORD_MODULE:
  case PM_TOKEN_KEYWORD_NEXT:
  case PM_TOKEN_KEYWORD_NIL:
  case PM_TOKEN_KEYWORD_NOT:
  case PM_TOKEN_KEYWORD_OR:
  case PM_TOKEN_KEYWORD_REDO:
  case PM_TOKEN_KEYWORD_RESCUE:
  case PM_TOKEN_KEYWORD_RESCUE_MODIFIER:
  case PM_TOKEN_KEYWORD_RETRY:
  case PM_TOKEN_KEYWORD_RETURN:
  case PM_TOKEN_KEYWORD_SELF:
  case PM_TOKEN_KEYWORD_SUPER:
  case PM_TOKEN_KEYWORD_THEN:
  case PM_TOKEN_KEYWORD_TRUE:
  case PM_TOKEN_KEYWORD_UNDEF:
  case PM_TOKEN_KEYWORD_UNLESS:
  case PM_TOKEN_KEYWORD_UNLESS_MODIFIER:
  case PM_TOKEN_KEYWORD_UNTIL:
  case PM_TOKEN_KEYWORD_UNTIL_MODIFIER:
  case PM_TOKEN_KEYWORD_WHEN:
  case PM_TOKEN_KEYWORD_WHILE:
  case PM_TOKEN_KEYWORD_WHILE_MODIFIER:
  case PM_TOKEN_KEYWORD_YIELD:
  case PM_TOKEN_KEYWORD___ENCODING__:
  case PM_TOKEN_KEYWORD___FILE__:
  case PM_TOKEN_KEYWORD___LINE__:
    return HIGHLIGHT_KEYWORD;

  /* Strings and string-like literals. EMBEXPR_BEGIN/END and EMBVAR are the
     `#{`, `}` and `#` of an interpolation: the punctuation belongs to the
     string, what is between them is lexed as ordinary code and stays 0. */
  case PM_TOKEN_STRING_BEGIN:
  case PM_TOKEN_STRING_CONTENT:
  case PM_TOKEN_STRING_END:
  case PM_TOKEN_HEREDOC_START:
  case PM_TOKEN_HEREDOC_END:
  case PM_TOKEN_CHARACTER_LITERAL:
  case PM_TOKEN_BACKTICK:
  case PM_TOKEN_PERCENT_LOWER_W:
  case PM_TOKEN_PERCENT_UPPER_W:
  case PM_TOKEN_PERCENT_LOWER_I:
  case PM_TOKEN_PERCENT_UPPER_I:
  case PM_TOKEN_PERCENT_LOWER_X:
  case PM_TOKEN_WORDS_SEP:
  case PM_TOKEN_EMBEXPR_BEGIN:
  case PM_TOKEN_EMBEXPR_END:
  case PM_TOKEN_EMBVAR:
  case PM_TOKEN_REGEXP_BEGIN:
  case PM_TOKEN_REGEXP_END:
    return HIGHLIGHT_STRING;

  /* Comments */
  case PM_TOKEN_COMMENT:
  case PM_TOKEN_EMBDOC_BEGIN:
  case PM_TOKEN_EMBDOC_LINE:
  case PM_TOKEN_EMBDOC_END:
    return HIGHLIGHT_COMMENT;

  /* Numbers */
  case PM_TOKEN_INTEGER:
  case PM_TOKEN_INTEGER_IMAGINARY:
  case PM_TOKEN_INTEGER_RATIONAL:
  case PM_TOKEN_INTEGER_RATIONAL_IMAGINARY:
  case PM_TOKEN_FLOAT:
  case PM_TOKEN_FLOAT_IMAGINARY:
  case PM_TOKEN_FLOAT_RATIONAL:
  case PM_TOKEN_FLOAT_RATIONAL_IMAGINARY:
    return HIGHLIGHT_NUMBER;

  /* Symbols. LABEL is `key:` whole; SYMBOL_BEGIN is only the `:` of `:sym`, and
     the name after it is painted by the second pass (PM_SYMBOL_NODE). */
  case PM_TOKEN_SYMBOL_BEGIN:
  case PM_TOKEN_LABEL:
    return HIGHLIGHT_SYMBOL;

  /* Constants */
  case PM_TOKEN_CONSTANT:
    return HIGHLIGHT_CONSTANT;

  /* Variables */
  case PM_TOKEN_INSTANCE_VARIABLE:
  case PM_TOKEN_CLASS_VARIABLE:
  case PM_TOKEN_GLOBAL_VARIABLE:
    return HIGHLIGHT_VARIABLE;

  default:
    return HIGHLIGHT_DEFAULT;
  }
}

static void
highlight_callback(void *data, pm_parser_t *parser, pm_token_t *token)
{
  highlight_data_t *hd = (highlight_data_t *)data;
  uint8_t category = token_type_to_category(token->type);
  if (category == HIGHLIGHT_DEFAULT) return;

  size_t start = (size_t)(token->start - parser->start);
  size_t end   = (size_t)(token->end   - parser->start);
  if (start > hd->size) return;
  if (end > hd->size) end = hd->size;
  for (size_t i = start; i < end; i++) hd->map[i] = category;
}

/* Paints one region of the map, as family-mruby's highlight_region does. */
static void
highlight_region(highlight_data_t *hd, const uint8_t *loc_start,
                 const uint8_t *loc_end, uint8_t category)
{
  if (loc_start == NULL || loc_end == NULL || loc_start >= loc_end) return;
  size_t start = (size_t)(loc_start - hd->source);
  size_t end   = (size_t)(loc_end   - hd->source);
  if (start > hd->size) return;
  if (end > hd->size) end = hd->size;
  for (size_t i = start; i < end; i++) hd->map[i] = category;
}

/*
 * The second pass, moved from family-mruby unchanged: what a token type cannot
 * say, the tree can. It runs after the lexer and overwrites what the lexer
 * wrote, which is family-mruby's order too.
 *
 * - PM_CALL_NODE:   the method name (message_loc), unless the call is a bare
 *                   identifier with no receiver and no arguments -- `x` in
 *                   `x = 1; p x` parses as a call but reads as a variable
 * - PM_DEF_NODE:    the name being defined (name_loc), operators included
 * - PM_SYMBOL_NODE: the whole symbol, `:` and quotes included
 */
static bool
highlight_visit_node(const pm_node_t *node, void *data)
{
  highlight_data_t *hd = (highlight_data_t *)data;

  switch (PM_NODE_TYPE(node)) {
  case PM_CALL_NODE: {
    const pm_call_node_t *call = (const pm_call_node_t *)node;
    if (call->base.flags & PM_CALL_NODE_FLAGS_VARIABLE_CALL) break;
    highlight_region(hd, call->message_loc.start, call->message_loc.end, HIGHLIGHT_METHOD);
    break;
  }
  case PM_DEF_NODE: {
    const pm_def_node_t *def = (const pm_def_node_t *)node;
    highlight_region(hd, def->name_loc.start, def->name_loc.end, HIGHLIGHT_METHOD);
    break;
  }
  case PM_SYMBOL_NODE: {
    const pm_symbol_node_t *sym = (const pm_symbol_node_t *)node;
    const uint8_t *start = sym->opening_loc.start;
    const uint8_t *end   = sym->value_loc.end;
    if (sym->closing_loc.end != NULL && sym->closing_loc.end > end) end = sym->closing_loc.end;
    if (start == NULL) start = sym->value_loc.start;
    highlight_region(hd, start, end, HIGHLIGHT_SYMBOL);
    break;
  }
  default:
    break;
  }

  return true;
}

/* Fills out[0..len) with one category byte per byte of src. `out` must have room
   for len bytes. A source that does not parse still gets a map: pm_parse
   recovers, the lex callback has already written every token it reached, and the
   tree it does build (a broken `def foo(` still has its DefNode) is walked as
   usual. */
void
sabiruby_mrc_highlight(const uint8_t *src, size_t len, uint8_t *out)
{
  if (!src || !out || len == 0) return;
  memset(out, HIGHLIGHT_DEFAULT, len);

  highlight_data_t hd = { out, len, src };

  pm_parser_t parser;
  pm_parser_init(&parser, src, len, NULL);
  pm_lex_callback_t lex_cb = { &hd, highlight_callback };
  parser.lex_callback = &lex_cb;

  pm_node_t *root = pm_parse(&parser);
  pm_visit_node(root, highlight_visit_node, &hd);
  pm_node_destroy(&parser, root);
  pm_parser_free(&parser);
}

void
sabiruby_mrc_free(void *p)
{
  free(p);
}

const char *
sabiruby_mrc_version(void)
{
  return "mruby 4.1.0-rc2 (c17ffcc24), Prism " PRISM_VERSION;
}

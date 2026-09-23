# Wide integers where mruby 4.1.0-rc2 (and -rc) answers something it does not answer for a
# plain Integer. Each one is a slip in mruby-bigint, not a decision, so SabiRuby
# keeps the meaning the same at both widths (docs/design/gems.md, "Deviations kept"):
#   ~x      `mrb_bint_rev` negates and then takes one off the MAGNITUDE
#           (`mpz_sub_int` ignores the sign), so it answers -(x-1) for x > 0.
#   x >> n  `mpz_div_2exp` shifts the magnitude, so a negative value rounds
#           toward zero instead of toward -infinity as `Integer#>>` does.
#   x.div(f), x % f   `mrb_bint_div` multiplies by the Float instead of dividing,
#           and `mrb_bint_mod` takes `fmod`, which truncates where `Integer#%`
#           floors.
#   x.dup   `mrb_obj_dup` copies an RInteger as an empty object, so an Integer
#           that is wide in the reference's build but immediate here comes back
#           as 0 there (the reference prints 0 for (2**62).dup).
# expected-from: CRuby 3.2 (run: ruby 3.2, the values below are its output)
p ~(2**64)
p((-(2**64) - 1) >> 1)
p((2**64).div(2.0))
p((2**64) % -3.0)
p((-(2**64)) % 3.0)
p((2**62).dup)

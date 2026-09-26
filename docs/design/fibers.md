# Fibers (mruby-fiber) without a second host stack

mruby switches fibers by swapping `mrb->c` (a `mrb_context`: register stack +
callinfo stack + status). SabiRuby does the same with `Vm::contexts`; the
running context's `stack`/`ci` are the `Vm` fields and are swapped in and out
(`switch_context`, three `mem::swap`s). Nothing is copied on a switch.

## The two ways a switch happens

mruby's `fiber_switch` has a `vmexec` flag; both cases exist here.

* **From bytecode** (`f.resume` compiled as a SEND): the native `Fiber#resume`
  switches `Vm::cur` and returns. The instruction loop that called it simply
  carries on with the new context's frames — no host recursion. A fiber may be
  resumed like this only from a frame that is not under a native call
  (`direct_send` is true: the native was invoked by a SEND, not by `funcall`).
* **From native code** (`Vm::fiber_resume`, mruby `mrb_fiber_resume`): the
  fiber runs in a nested `run_loop` on the host stack (`vmexec = true`). When
  it yields or finishes, that nested loop returns the value to the native.

## Where the value goes

When the native `resume`/`yield`/`transfer` returns after a switch, the
instruction loop must not write its result into the old context's register.
`call_native_direct` notices `cur` changed and instead delivers the value to
`Context::pending_reg` of the new current context: the register of the
`resume`/`yield` call that context is suspended in. mruby gets the same effect
by leaving the C-function callinfo pushed on the suspended context and writing
the value to its `stack[0]` on return.

If the fiber that yielded had been resumed by native code (`vmexec`), the
yield sets `loop_exit` and the loop returns the value instead (mruby: the
resumer's callinfo gets `CINFO_RESUMED` and `mrb_vm_exec` returns).

## Run loops and contexts

`run_loop_ctx(lc, stop_depth)` stops when frame `stop_depth` of context `lc`
returns. Frames of any other context the loop is switched into never stop it;
the only event handled there is the fiber's entry frame returning
(`fiber_terminate`: status Terminated, switch to `prev` or root, deliver the
value or, for `vmexec`, end the nested loop). An exception leaving a fiber
terminates it the same way and the handler search continues in the resumer
(mruby `L_FTOP`); with `vmexec` it comes back as `Err` from the nested loop.

## Native boundaries

A fiber cannot be switched while any frame of its context was pushed by native
code (`Cci::Skip`; mruby `fiber_check_cfunc`, "can't cross C function boundary"),
except the entry frame at index 0 (mruby's `cibase`, as in
`task_across_c_boundary`). `Fiber.yield` reached through `funcall` (native →
native) is refused for the same reason; `transfer` checks the whole chain of
resumers.

This is why `send`/`__send__` issued by bytecode are not calls of the native
`send` function: the VM shifts the arguments and dispatches the named method in
the same frame (`op_send_redirect`, mruby `mrb_f_send` → `mrb_exec_irep`).
`Enumerator#each` is `@obj.__send__(@meth, ...)`, and its block does
`Fiber.yield` — with a native `send` in between every external iterator would fail.

A `method_missing` written in Ruby is dispatched the same way, and for the same
reason: `op_send`'s fallback writes the missing name in as the first argument and
re-dispatches in the frame the call was already in (mruby `prepare_missing`), so the
body is an ordinary frame and can `Fiber.yield` — or park a task on `Queue#pop` — out
of it. The basic `method_missing` written in C, and `Vm::funcall`'s own fallback
(nested, as `mrb_funcall` is), keep the boundary.

The same move — a frame where the SEND's value goes, instead of a nested loop — is what keeps
`instance_exec` and its relatives, `Method#call`, `public_send`, `Class#new`, the string `eval`s,
`catch` and the block-taking natives DSLs use (`index { }`, `sort { }`, `Array.new(n) { }`,
`Class.new { }`, a Hash's default proc, …) off the boundary: [`wait-anywhere.md`](wait-anywhere.md),
which also lists what is still a boundary and the error that names it.

The three index opcodes are the same story. `OP_GETIDX`, `OP_GETIDX0` and `OP_SETIDX`
answer an Array, Hash or String themselves — while that class still carries the `[]`
they stand in for — and *send* everything else in the frame the call was made in
(mruby's `L_SEND_SYM`), so a `[]` or `[]=` written in Ruby can `Fiber.yield` or park a
task on a queue out of itself; `e[:Transform]` in rubevy is exactly that.

## Environments

An `REnv` that is still attached records its context (`EnvData::ctx`), so a
closure created in one fiber and read from another finds the right stack.
Detaching on frame pop is per context and unchanged.

## What passes

`mrbgems/mruby-fiber/test/fiber.rb` (21) and `fiber2.rb` (4, with the six
helpers of `fibertest.c` provided by `mrbtest.rs`), and
`mruby-enumerator/test/enumerator.rb` (52) all pass; see `docs/verification/mrbtest.md`.
`Fiber#to_s` omits the `file:line` part (no debug info is read).

//! Resuming a checkpoint image written by C mruby (the feature `checkpoint`, off by default).
//!
//! An image is the state of a running program — its objects, its call frames and its register
//! stacks (the root's, each Task's and each Fiber's), the scheduler's Tasks — written by an
//! mruby that stopped at an instruction boundary (`Checkpoint.save` or `Checkpoint.suspend` in
//! the program, or a request from outside). It is a logical graph: objects are numbered and refer
//! to each other by number, symbols travel by name, code (ireps) by content, and what the
//! writer's VM had before the program ran (its classes, the methods of its mrblib, its C
//! functions) by a name that is looked up here (`names`). The program then goes on from the
//! instruction after the call, here.
//!
//! What this adds to the VM is reached only through [`Vm::checkpoint_resume`]: nothing in the
//! instruction loop, the collector or the value representation changes, and a build without
//! the feature has none of this module. The plan, the format as this reader takes it and what
//! is refused are in `docs/plans/checkpoint-plan.md`.

mod build;
mod image;
mod module;
mod names;

use alloc::string::String;

use crate::error::VmResult;
use crate::value::Value;
use crate::vm::Vm;

pub use module::data_types;

impl Vm {
    /// Defines the `Checkpoint` module (`save`, `suspend`, `resumed?`, `Checkpoint::Error`,
    /// `Checkpoint::Box`) if it is not there. [`Vm::checkpoint_resume`] does it first; a host
    /// that runs a program written for the writer from its start calls it to give that program
    /// the module. `save` and `suspend` here write nothing (see the `module` source).
    pub fn checkpoint_install(&mut self) {
        module::install(self);
    }

    /// Restores `image` into this VM and runs the program from where it stopped to its end;
    /// answers the value of its top level. The VM must be idle (made by [`Vm::new`] and
    /// [`Vm::load_mrblib`], nothing running). An image that cannot be restored here is
    /// refused with `Checkpoint::Error` and a message that says what is missing; the VM is
    /// left idle then (no frame), but a refusal that comes after the changes to the permanent
    /// classes were applied leaves those changes, so a host drops that VM.
    pub fn checkpoint_resume(&mut self, image: &[u8]) -> VmResult<Value> {
        self.checkpoint_install();
        let img = match image::parse(image) {
            Ok(i) => i,
            Err(e) => return Err(module::error(self, &alloc::format!("restore failed: {e}"))),
        };
        let plan = match build::build(self, &img) {
            Ok(p) => p,
            Err(e) => return Err(module::error(self, &alloc::format!("restore failed: {e}"))),
        };
        module::set_resumed(self);
        // what was running goes on: the context it stood in, or the rest of a Task's slice and
        // then whoever ran the scheduler (`build::Plan`)
        if let build::Plan::Slice { task, start, ended, entry } = plan {
            crate::builtins::ext_task::checkpoint_slice(self, task, start, ended);
            match entry {
                build::Entry::Host => {
                    let r = crate::builtins::ext_task::checkpoint_run_loop(self);
                    self.stack.clear();
                    return r;
                }
                build::Entry::Run => {}
                build::Entry::Pass(reg) => {
                    if reg < self.stack.len() { self.stack[reg] = crate::value::Slot::NIL; }
                }
            }
        }
        self.checkpoint_run()
    }

    /// Whether a permanent name of the writer (one line of its `checkpoint names`) has a
    /// counterpart here: `Ok` says what it found, `Err` why not.
    pub fn checkpoint_name(&mut self, name: &str) -> Result<&'static str, String> {
        self.checkpoint_install();
        names::check(self, name)
    }

    /// The header of an image: the format version and the writer's name for itself.
    pub fn checkpoint_header(image: &[u8]) -> Result<(u16, String), String> {
        image::parse(image).map(|i| (i.version, i.writer))
    }
}

/// A summary of an image for tools: (irep records, object records, changes, frames of all the
/// contexts).
pub fn image_counts(image: &[u8]) -> Result<(usize, usize, usize, usize), String> {
    let i = image::parse(image)?;
    Ok((i.ireps.len(), i.objs.len(), i.patches.len(), i.ctxs.iter().map(|c| c.frames.len()).sum()))
}

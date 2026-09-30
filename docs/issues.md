- [ ] adding a second folder to a session does not work
- Chat
  - [ ] ability to navigate from edit to working copy version of the edited file
  - [ ] group tool calls into a subtree
- Claude
  - [ ] when downgrades to opus, it's not visible in chat and the model is not controllable afterwards
  - [ ] implement alternative chat host based on claude code plugin api, to allow steering terminal agents
- [x] panic: FIXED at the root — himarkdown used to parse markdown
  over a VIRTUAL trailing newline (the vendored grammar treated its
  absence as ERROR-heading / unclosed-fence), so every fresh tree
  ended one byte past its source; `structdiff::convert` sliced with
  that range and the normalize-lane panic aborted the app (nounwind
  boundary). The grammar now treats EOF as a line ending (mdparser
  grammar.json `_eof` on atx headings / fence close / empty list
  item; scanner.c fence-close, setext and minus matchers — see
  grammar/README.md) and the virtual newline is gone; trees can no
  longer outrun their text. `structdiff::structural` additionally
  REFUSES an out-of-bounds tree (degrade to Myers, beside the
  error-heavy gate) so a regressing producer can never abort the
  app again. Tests: `mdparser/tests/eof_line_ending.rs`,
  `structdiff/tests/tree_bounds.rs`, himarkdown assist EOF cases.
``` 
thread '<unnamed>' (11829601) panicked at frontend/structdiff/src/convert.rs:116:29:
end byte index 403 is out of bounds of `- [ ] adding a second folder to a session does not work
- Chat
  - [ ] ability to navigate from edit to working copy version of the edited file
  - [ ] group tool calls into a subtree
- Claude
  - [ ] when downgrades to opus, it's not visible in chat and t`[...]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

thread '<unnamed>' (11829601) panicked at /rustc/59807616e1fa2540724bfbac14d7976d7e4a3860/library/core/src/panicking.rs:225:5:
panic in a function that cannot unwind
stack backtrace:
   0:        0x1073a6e4c - <<std[bb513d90a5cee88a]::sys::backtrace::BacktraceLock>::print::DisplayBacktrace as core[f63e075e1375d836]::fmt::Display>::fmt
   1:        0x1073e0f68 - core[f63e075e1375d836]::fmt::write
   2:        0x1073b6b7c - <std[bb513d90a5cee88a]::sys::stdio::unix::Stderr as std[bb513d90a5cee88a]::io::Write>::write_fmt
   3:        0x10737f6bc - std[bb513d90a5cee88a]::panicking::default_hook::{closure#0}
   4:        0x10739af7c - std[bb513d90a5cee88a]::panicking::default_hook
   5:        0x1065a3ca8 - host_discovery::logging::init::{{closure}}::{{closure}}::h459af7b546e71d9b
   6:        0x10739b2e0 - std[bb513d90a5cee88a]::panicking::panic_with_hook
   7:        0x10737f794 - std[bb513d90a5cee88a]::panicking::panic_handler::{closure#0}
   8:        0x1073764b8 - std[bb513d90a5cee88a]::sys::backtrace::__rust_end_short_backtrace::<std[bb513d90a5cee88a]::panicking::panic_handler::{closure#0}, !>
   9:        0x107380a4c - __rustc[b7974e8690430dd9]::rust_begin_unwind
  10:        0x107559b94 - core[f63e075e1375d836]::panicking::panic_nounwind_fmt
  11:        0x107559ae0 - core[f63e075e1375d836]::panicking::panic_nounwind
  12:        0x107559c70 - core[f63e075e1375d836]::panicking::panic_cannot_unwind
  13:        0x1045ddd58 - _himark_run_pending
  14:        0x1045a3cd0 - $sIegh_IeyBh_TR
  15:        0x18de6ca28 - <unknown>
  16:        0x18de864b0 - <unknown>
  17:        0x18de75030 - <unknown>
  18:        0x18de75b2c - <unknown>
  19:        0x18de7fe34 - <unknown>
  20:        0x18de7f734 - <unknown>
  21:        0x18e023ec0 - __pthread_wqthread_setup
thread caused non-unwinding panic. aborting.
```

- [ ] file-tree usability:
    another usability gap in file tree is the lack of ability to create and delete and rename files. let's add a popup menu (see how overlays are built and the combo box, introduce MenuView and PopupMenuView). renaming or creating a file creates/replaces a tree row with a small editor, finishing on enter and validating the input to be a valid file name. focus loss leads to the editor removal/rename-cancelled. for the roots, there should be a menu item to remove a directory from the session. 
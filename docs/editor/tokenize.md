# tokenize — restartable rope rewriting

`frontend/tokenize`: one generic function for incrementally rewriting
part of a rope with the output of a LINEAR process. Given

- a linear emitter of elements (an `Iterator<T>` — a lexer, a
  soft-wrapper),
- a notion of places where that process can be safely restarted,
- the damaged places in the rope, and
- an alignment metric plus a way to cut an element's tail by it,

the rewrite starts at the damage, runs the process forward, and stops
at the first safepoint past its budget — the rest of the rope is
untouched and the process can resume there later. Incrementality
falls out of the safepoint notion instead of being re-invented per
consumer.

## The uses it is shaped for

- **Tokenization.** A lexer yields tokens from a restartable state.
  After an edit that expanded or collapsed tokens without re-lexing,
  seek to the first edit point, scan back to the closest safepoint,
  and lex forward from there. The process may stop at any later
  safepoint when the re-lexing budget is spent, and safely continue
  in a later slice of work.
- **Soft-wrapping — the consumer in the tree.** When wrapping lays a
  paragraph out, every emitted soft line is a safepoint: it starts at
  column 0 and nothing before it affects it. After an edit, wrapping
  restarts at the modified soft line's beginning and runs until it
  reaches the start of SOME soft line with its budget spent — cut the
  rest, continue later. This is how `DocumentLayout::repair_region`
  (`frontend/editor/src/document_layout.rs`) maintains the layout
  rope: `LayoutSafepoints` implements the policy over
  `LayoutElement`s.

## The API

```rust
pub trait Safepoint<T> {
    fn is_safepoint(&self, element: &T) -> bool;
    /// Cut an element at `offset` of the alignment metric (local
    /// within the element).
    fn cut(&self, element: T, offset: u32) -> T;
    /// Whether a stop may land ON this old element (defaults true).
    fn can_resume(&self, _element: &T, _alignment: u32) -> bool { true }
}

pub fn rewrite<T, M, Tokens, Policy, Stop>(
    cursor: &mut Cursor<T, M>,     // where the rewrite starts
    tokens: Tokens,                 // the linear process's output
    policy: &Policy,                // Safepoint<T>
    alignment_metric: MetricId,     // what "the same place" means
    stop: Stop,                     // mut FnMut(M::Metrics) -> bool
) -> RewriteReport<M::Metrics>
```

`rewrite` consumes tokens from the cursor's position and stops only
when ALL THREE hold: the token is a safepoint, the `stop` predicate
says the budget is spent, AND the old rope holds an aligned element
at that position that is itself a safepoint and answers `can_resume`
— so the spliced-in prefix always meets the surviving tail at a
boundary both sides agree on. It then deletes the replaced old
elements — an old element STRADDLING the stop is cut by the alignment
metric and its tail re-joins the replacement — splices in the
accumulated replacement, and reports what happened:

```rust
pub struct RewriteReport<M> {
    pub old_elements_replaced: u32,
    pub replacement_elements: u32,
    pub stopped: bool,   // budget stop — the caller schedules a resume
    pub location: M,     // where the rewrite ended
}
```

## Where the budget lives

Deliberately NOT in `rewrite`: the `stop` predicate only says where a
stop is allowed, and the budget rides the TOKEN ITERATOR. Layout
repair's stop closure is `|location| metric_at(location, BYTES) >
repair_start` — "any safepoint past the resume point may end the
slice" — while its token source (`BudgetedItems`) enforces the actual
budget: a height budget OR a 512-item cap, whichever exhausts first,
and once exhausted it still drains zero-byte items so the slice never
ends mid-position. The separation keeps `rewrite` free of any policy
about what a budget measures.

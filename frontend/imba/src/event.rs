// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use skia_safe::{Canvas, Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Escape,
    Tab,
    Home,
    End,
    PageUp,
    PageDown,

    Delete,

    F(u8),

    Char(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,

    pub command: bool,
}

/// The pointer shape the shell should show — a hit test's answer
/// (`EventResult::Pointer`), derived per pointer event and applied by
/// the shell right away. Never stored: a stored shape is state to
/// sync, and it stuck as a hand whenever the view that set it
/// stopped being hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PointerShape {
    #[default]
    Default,

    /// The pointing hand — something under the pointer follows on
    /// click.
    Pointer,
}

impl PointerShape {
    /// The shape a merge keeps: the hand under one child beats the
    /// arrows its missed siblings answer.
    pub fn or(self, other: PointerShape) -> PointerShape {
        match self {
            PointerShape::Default => other,
            shape => shape,
        }
    }
}

#[derive(Clone, Copy)]
pub enum Event<'a> {
    Paint {
        canvas: &'a Canvas,

        focused: bool,
    },
    MouseDown {
        point: Point,
        button: MouseButton,

        mods: Modifiers,

        count: u8,
    },

    MouseDrag {
        point: Point,
        mods: Modifiers,
    },

    MouseUp {
        point: Point,
    },

    MouseMove {
        point: Point,
        mods: Modifiers,
    },

    HitTest {
        point: Point,
        miss: bool,
        mods: Modifiers,
    },
    Scroll {
        point: Point,
        delta_x: f32,
        delta_y: f32,

        gesture: &'a ScrollGesture,
    },
    TextInput {
        text: &'a str,
    },
    KeyDown {
        key: Key,
        mods: Modifiers,
    },

    AnimationClock {
        now: crate::anim::AnimationClock,
    },

    /// The engine's synchronous reconcile pulse: dispatched between a
    /// perform batch and the paint whenever a view raised the settle
    /// bit (`Effects::settle`). Anchor-holding widgets answer with
    /// exact-placement reveals; nothing else should react.
    Settle,

    ThemeChanged,

    UserEvent(&'a dyn std::any::Any),
}

impl Event<'_> {
    /// The cursor left the window: a HitTest beyond any component's
    /// reach, so hover state — tooltips, hover popups — lets go.
    /// Every shell issues this on its cursor-exit notification;
    /// nothing else ever arrives from outside the window.
    pub fn window_left() -> Event<'static> {
        Event::HitTest {
            point: Point::new(-1.0e6, -1.0e6),
            miss: true,
            mods: Modifiers::default(),
        }
    }

    pub fn translated(self, dx: f32, dy: f32) -> Self {
        match self {
            Event::MouseDown {
                mut point,
                button,
                mods,
                count,
            } => {
                point.x += dx;
                point.y += dy;
                Event::MouseDown {
                    point,
                    button,
                    mods,
                    count,
                }
            }
            Event::MouseDrag { mut point, mods } => {
                point.x += dx;
                point.y += dy;
                Event::MouseDrag { point, mods }
            }
            Event::MouseMove { mut point, mods } => {
                point.x += dx;
                point.y += dy;
                Event::MouseMove { point, mods }
            }
            Event::HitTest {
                mut point,
                miss,
                mods,
            } => {
                point.x += dx;
                point.y += dy;
                Event::HitTest { point, miss, mods }
            }
            Event::MouseUp { mut point } => {
                point.x += dx;
                point.y += dy;
                Event::MouseUp { point }
            }
            Event::Scroll {
                mut point,
                delta_x,
                delta_y,
                gesture,
            } => {
                point.x += dx;
                point.y += dy;
                Event::Scroll {
                    point,
                    delta_x,
                    delta_y,
                    gesture,
                }
            }
            event => event,
        }
    }
}

#[derive(Default)]
pub struct ScrollGesture {
    owner: std::cell::Cell<Option<ScrollSurfaceId>>,
}

impl ScrollGesture {
    pub fn begin(&self) {
        self.owner.set(None);
    }

    pub fn claims(&self, surface: ScrollSurfaceId) -> bool {
        match self.owner.get() {
            None => {
                self.owner.set(Some(surface));
                true
            }
            Some(owner) => owner == surface,
        }
    }

    pub fn owned_by(&self, surface: ScrollSurfaceId) -> bool {
        self.owner.get() == Some(surface)
    }

    pub fn owned_by_other(&self, surface: ScrollSurfaceId) -> bool {
        self.owner.get().is_some_and(|owner| owner != surface)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScrollSurfaceId(u64);

impl ScrollSurfaceId {
    pub fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }

    pub fn keyed(key: u64) -> Self {
        Self(key | 1 << 63)
    }
}

/// What a dispatch answered. NORMALIZED: either nobody answered,
/// or someone did — and an answer is ONE product of everything it
/// can carry (commands beside a reveal beside the pointer's shape),
/// so no half is ever lost to a sibling's half in a merge. The old
/// shape was one variant per combination; a canvas revealing its
/// row lost the ask to its own animation tick, every tick.
pub enum EventResult<Command> {
    /// Nothing answered: the event falls through to whatever sits
    /// beneath.
    Ignored,
    Handled(Handled<Command>),
}

/// The answer's payload.
pub struct Handled<Command> {
    pub commands: Vec<Command>,

    /// A scroll ask — the nearest scroller consumes it en route;
    /// on the descrollered roads it travels all the way to the
    /// HOST.
    pub reveal: Option<Reveal>,

    /// A hit test's answer: the shape the pointer takes over what
    /// it hit. DERIVED per dispatch — the shell applies it to the
    /// platform cursor and nothing stores it, so there is no stale
    /// shape to sync (a stored one stuck as a hand whenever the
    /// view that set it stopped being hit).
    pub pointer: Option<PointerShape>,
}

impl<Command> Default for Handled<Command> {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            reveal: None,
            pointer: None,
        }
    }
}

impl<Command> EventResult<Command> {
    pub fn map<ParentCommand>(
        self,
        f: impl Fn(Command) -> ParentCommand,
    ) -> EventResult<ParentCommand> {
        match self {
            EventResult::Ignored => EventResult::Ignored,
            EventResult::Handled(handled) => EventResult::Handled(Handled {
                commands: handled.commands.into_iter().map(f).collect(),
                reveal: handled.reveal,
                pointer: handled.pointer,
            }),
        }
    }

    pub fn reveal_translated(self, dx: f32, dy: f32) -> EventResult<Command> {
        match self {
            EventResult::Handled(mut handled) => {
                handled.reveal = handled.reveal.map(|reveal| reveal.translated(dx, dy));
                EventResult::Handled(handled)
            }
            result => result,
        }
    }

    pub fn merge(self, other: EventResult<Command>) -> EventResult<Command> {
        match (self, other) {
            (EventResult::Ignored, other) => other,
            (result, EventResult::Ignored) => result,
            (EventResult::Handled(mut first), EventResult::Handled(second)) => {
                match first.commands.is_empty() {
                    true => first.commands = second.commands,
                    false => first.commands.extend(second.commands),
                }
                first.reveal = first.reveal.or(second.reveal);
                // A hit test's answer survives the merge: the one
                // child that was hit names the shape, its missed
                // siblings answer the arrow.
                first.pointer = match (first.pointer, second.pointer) {
                    (Some(mine), Some(theirs)) => Some(mine.or(theirs)),
                    (mine, theirs) => mine.or(theirs),
                };
                EventResult::Handled(first)
            }
        }
    }
}

impl<Command> EventResult<Command> {
    /// Consumed, nothing to say.
    pub fn handled() -> Self {
        EventResult::Handled(Handled::default())
    }

    pub fn command(command: Command) -> Self {
        EventResult::Handled(Handled {
            commands: vec![command],
            ..Default::default()
        })
    }

    pub fn commands(commands: Vec<Command>) -> Self {
        EventResult::Handled(Handled {
            commands,
            ..Default::default()
        })
    }

    pub fn reveal(reveal: Reveal) -> Self {
        EventResult::Handled(Handled {
            reveal: Some(reveal),
            ..Default::default()
        })
    }

    pub fn pointer(shape: PointerShape, commands: Vec<Command>) -> Self {
        EventResult::Handled(Handled {
            commands,
            pointer: Some(shape),
            ..Default::default()
        })
    }

    /// The commands, whatever else rode along — Ignored answers
    /// none.
    pub fn into_commands(self) -> Vec<Command> {
        match self {
            EventResult::Ignored => Vec::new(),
            EventResult::Handled(handled) => handled.commands,
        }
    }

    /// The FIRST command, if any — assertion sugar.
    pub fn into_command(self) -> Option<Command> {
        self.into_commands().into_iter().next()
    }

    /// The payload, if anyone answered.
    pub fn into_handled(self) -> Option<Handled<Command>> {
        match self {
            EventResult::Ignored => None,
            EventResult::Handled(handled) => Some(handled),
        }
    }
}

/// A parameterized reveal: WHAT to show and HOW to place it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reveal {
    pub rect: Rect,
    pub placement: Placement,
    pub motion: Motion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Make the rect visible; if it is not, aim its top at the golden
    /// section. Today's only behavior.
    EnsureVisible,
    /// The rect's origin becomes the viewport's top-left, EXACTLY —
    /// viewport preservation and pixel-true restores.
    TopLeftAt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// Jump when near, glide when far. Today's only behavior.
    Auto,
    /// Always jump: the correction must land in the current frame.
    Jump,
}

impl Reveal {
    pub fn visible(rect: Rect) -> Self {
        Self {
            rect,
            placement: Placement::EnsureVisible,
            motion: Motion::Auto,
        }
    }

    pub fn top_left_at(rect: Rect) -> Self {
        Self {
            rect,
            placement: Placement::TopLeftAt,
            motion: Motion::Jump,
        }
    }

    pub fn translated(mut self, dx: f32, dy: f32) -> Self {
        self.rect = self.rect.with_offset((dx, dy));
        self
    }
}

pub fn reveal_satisfied(viewport: Rect, rect: Rect) -> bool {
    let vertically = match rect.height() <= viewport.height() {
        true => rect.top >= viewport.top && rect.bottom <= viewport.bottom,
        false => rect.top >= viewport.top && rect.top < viewport.bottom,
    };
    let horizontally = match rect.width() <= viewport.width() {
        true => rect.left >= viewport.left && rect.right <= viewport.right,
        false => rect.left >= viewport.left && rect.left < viewport.right,
    };
    vertically && horizontally
}

pub fn reveal_scroll_target(current: f32, height: f32, top: f32, bottom: f32) -> f32 {
    let oversized = bottom - top > height;
    let visible = match oversized {
        false => top >= current && bottom <= current + height,

        true => top >= current && top < current + height,
    };
    if visible {
        return current;
    }
    const GOLDEN_SECTION: f32 = 0.381_966;
    top - height * GOLDEN_SECTION
}

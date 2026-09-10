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
    },

    HitTest {
        point: Point,
        miss: bool,
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

    ThemeChanged,

    UserEvent(&'a dyn std::any::Any),
}

impl Event<'_> {
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
            Event::MouseMove { mut point } => {
                point.x += dx;
                point.y += dy;
                Event::MouseMove { point }
            }
            Event::HitTest { mut point, miss } => {
                point.x += dx;
                point.y += dy;
                Event::HitTest { point, miss }
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
        self.owner
            .get()
            .is_some_and(|owner| owner != surface)
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

pub enum EventResult<Command> {
    Ignored,
    Handled,
    Command(Command),

    Commands(Vec<Command>),

    Reveal(Rect),
}

impl<Command> EventResult<Command> {
    pub fn map<ParentCommand>(
        self,
        f: impl Fn(Command) -> ParentCommand,
    ) -> EventResult<ParentCommand> {
        match self {
            EventResult::Ignored => EventResult::Ignored,
            EventResult::Handled => EventResult::Handled,
            EventResult::Command(command) => EventResult::Command(f(command)),
            EventResult::Commands(commands) => {
                EventResult::Commands(commands.into_iter().map(f).collect())
            }
            EventResult::Reveal(rect) => EventResult::Reveal(rect),
        }
    }

    pub fn reveal_translated(self, dx: f32, dy: f32) -> EventResult<Command> {
        match self {
            EventResult::Reveal(rect) => EventResult::Reveal(rect.with_offset((dx, dy))),
            result => result,
        }
    }

    pub fn merge(self, other: EventResult<Command>) -> EventResult<Command> {
        let mut reveal = None;
        let mut commands = match self {
            EventResult::Commands(commands) => commands,
            EventResult::Command(command) => vec![command],
            EventResult::Reveal(rect) => {
                reveal = Some(rect);
                Vec::new()
            }
            _ => Vec::new(),
        };
        match other {
            EventResult::Command(command) => commands.push(command),
            EventResult::Commands(more) => match commands.is_empty() {
                true => commands = more,
                false => commands.extend(more),
            },
            EventResult::Reveal(rect) => reveal = reveal.or(Some(rect)),
            _ => {}
        }
        match (commands.is_empty(), reveal) {
            (false, _) => EventResult::Commands(commands),
            (true, Some(rect)) => EventResult::Reveal(rect),
            (true, None) => EventResult::Ignored,
        }
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

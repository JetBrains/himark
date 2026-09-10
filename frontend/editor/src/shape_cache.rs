use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use crate::shaped_line::ShapedLine;

pub(crate) struct ShapeStamp {
    pub token: crate::document::DocumentToken,
    pub editor: crate::editor::EditorId,
    pub revision: u64,
    pub markup_generation: u64,
    pub theme: Arc<str>,
    pub width_bits: u32,
}

#[derive(PartialEq, Eq, Hash, Clone, Copy)]
struct Key {
    token: crate::document::DocumentToken,
    editor: crate::editor::EditorId,
    byte_start: u32,
}

struct Entry {
    shaped: Rc<ShapedLine>,
    byte_end: u32,
    revision: u64,
    markup_generation: u64,
    theme: Arc<str>,
    width_bits: u32,
    selected: bool,
    marked: Option<Range<u32>>,
    last_use: u64,
}

const CAPACITY: usize = 1024;

struct Cache {
    entries: HashMap<Key, Entry>,

    clock: u64,

    capacity: usize,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            capacity: CAPACITY,
        }
    }
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

pub(crate) fn begin_build() {
    CACHE.with(|cell| cell.borrow_mut().clock += 1);
}

pub(crate) fn shaped(
    stamp: &ShapeStamp,
    line: Range<u32>,
    selected: bool,
    marked: Option<Range<u32>>,
    shape: impl FnOnce() -> ShapedLine,
) -> Rc<ShapedLine> {
    CACHE.with(|cell| {
        {
            let mut cache = cell.borrow_mut();
            let clock = cache.clock;
            let key = Key {
                token: stamp.token,
                editor: stamp.editor,
                byte_start: line.start,
            };
            if let Some(entry) = cache.entries.get_mut(&key) {
                if entry.byte_end == line.end
                    && entry.revision == stamp.revision
                    && entry.markup_generation == stamp.markup_generation
                    && entry.width_bits == stamp.width_bits
                    && entry.selected == selected
                    && entry.marked == marked
                    && entry.theme == stamp.theme
                {
                    entry.last_use = clock;
                    return Rc::clone(&entry.shaped);
                }
            }
        }

        let shaped = Rc::new(shape());
        let mut cache = cell.borrow_mut();
        let clock = cache.clock;
        if cache.entries.len() >= cache.capacity {
            cache.entries.retain(|_, entry| entry.last_use + 4 >= clock);
            if cache.entries.len() >= cache.capacity {
                cache.capacity *= 2;
            }
        }
        cache.entries.insert(
            Key {
                token: stamp.token,
                editor: stamp.editor,
                byte_start: line.start,
            },
            Entry {
                shaped: Rc::clone(&shaped),
                byte_end: line.end,
                revision: stamp.revision,
                markup_generation: stamp.markup_generation,
                theme: stamp.theme.clone(),
                width_bits: stamp.width_bits,
                selected,
                marked,
                last_use: clock,
            },
        );
        shaped
    })
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn len() -> usize {
    CACHE.with(|cell| cell.borrow().entries.len())
}

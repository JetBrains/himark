use std::any::{Any, TypeId};
use std::cell::RefCell;

#[derive(Default)]
pub struct UiCtx {
    slots: RefCell<Vec<(TypeId, Box<dyn Any>)>>,
}

impl UiCtx {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set<T: 'static>(&self, value: T) {
        if self.get::<T>().is_none() {
            self.slots
                .borrow_mut()
                .push((TypeId::of::<T>(), Box::new(value)));
        }
    }

    pub fn get<T: 'static>(&self) -> Option<&T> {
        let slots = self.slots.borrow();
        for (id, slot) in slots.iter() {
            if *id == TypeId::of::<T>() {
                let reference: &T = slot.downcast_ref::<T>().expect("typeid matched");
                let pointer: *const T = reference;
                return Some(unsafe { &*pointer });
            }
        }
        None
    }

    pub fn env<T: 'static>(&self, init: impl FnOnce() -> T) -> &T {
        if let Some(value) = self.get::<T>() {
            return value;
        }
        self.set(init());
        self.get::<T>().expect("just set")
    }
}

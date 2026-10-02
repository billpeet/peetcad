//! A value that may arrive later.
//!
//! The browser's file pickers and IndexedDB are asynchronous, and the app has a single UI
//! thread that must not block. Operations that can finish later therefore hand back a
//! [`Pending`], which the UI polls once per frame. Natively the same operations finish
//! straight away and return an already resolved `Pending`.

use std::cell::RefCell;
use std::rc::Rc;

enum Slot<T> {
    Waiting,
    Ready(T),
    Taken,
}

/// The result of an operation that may finish later (always later on the web, where file
/// pickers and IndexedDB are asynchronous; immediately on native). Single-threaded: poll it
/// from the UI each frame.
///
/// Nothing wakes the UI when the value arrives, so keep repainting (for example with egui's
/// `request_repaint_after`) for as long as you hold a `Pending` that is not done.
pub struct Pending<T> {
    slot: Rc<RefCell<Slot<T>>>,
}

/// The writing end of a [`Pending`], from [`Pending::deferred`].
///
/// If it is dropped without calling [`Resolver::resolve`], the `Pending` waits forever.
pub struct Resolver<T> {
    slot: Rc<RefCell<Slot<T>>>,
}

impl<T> Pending<T> {
    /// A `Pending` whose value is already there.
    pub fn ready(value: T) -> Self {
        Self {
            slot: Rc::new(RefCell::new(Slot::Ready(value))),
        }
    }

    /// A `Pending` that is still running, and the [`Resolver`] that finishes it.
    pub fn deferred() -> (Self, Resolver<T>) {
        let slot = Rc::new(RefCell::new(Slot::Waiting));
        (Self { slot: slot.clone() }, Resolver { slot })
    }

    /// Takes the value once it's there. Returns `None` while still running (and after it
    /// was taken).
    pub fn take(&self) -> Option<T> {
        let mut slot = self.slot.borrow_mut();
        if !matches!(*slot, Slot::Ready(_)) {
            return None;
        }
        match std::mem::replace(&mut *slot, Slot::Taken) {
            Slot::Ready(value) => Some(value),
            Slot::Waiting | Slot::Taken => None,
        }
    }

    /// Whether the value arrived (taken or not).
    pub fn is_done(&self) -> bool {
        !matches!(*self.slot.borrow(), Slot::Waiting)
    }
}

impl<T> Resolver<T> {
    /// Delivers the value to the [`Pending`].
    pub fn resolve(self, value: T) {
        *self.slot.borrow_mut() = Slot::Ready(value);
    }
}

impl<T> std::fmt::Debug for Pending<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match *self.slot.borrow() {
            Slot::Waiting => "waiting",
            Slot::Ready(_) => "ready",
            Slot::Taken => "taken",
        };
        f.debug_struct("Pending").field("state", &state).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_value_is_taken_once() {
        let pending = Pending::ready(7);
        assert!(pending.is_done());
        assert_eq!(pending.take(), Some(7));
        assert_eq!(pending.take(), None);
        assert!(pending.is_done());
    }

    #[test]
    fn deferred_value_arrives_later() {
        let (pending, resolver) = Pending::deferred();
        assert!(!pending.is_done());
        assert_eq!(pending.take(), None);
        resolver.resolve("done".to_owned());
        assert!(pending.is_done());
        assert_eq!(pending.take().as_deref(), Some("done"));
        assert_eq!(pending.take(), None);
    }

    #[test]
    fn dropped_resolver_never_resolves() {
        let (pending, resolver) = Pending::<u8>::deferred();
        drop(resolver);
        assert!(!pending.is_done());
        assert_eq!(pending.take(), None);
    }
}

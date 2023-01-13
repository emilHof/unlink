//! This crate provides a lock free stack that supports concurrent `push`, `pop`, `peek`, and
//! `extend`.
//! ```
//! use unlink::Stack;
//! use std::thread;
//!
//! let stack = Stack::new();
//! thread::scope(|s| {
//!     let stack = &stack;
//!
//!     s.spawn(move || {
//!         for i in 0..100 {
//!             stack.push(i);
//!         }
//!     });
//!
//!     s.spawn(move || {
//!         for _ in 0..100 {
//!             stack.pop();
//!         }
//!     });
//!
//!     s.spawn(move || {
//!         for _ in 0..100 {
//!             let _ = stack.peek();
//!         }
//!     });
//!
//!     s.spawn(move || {
//!         for i in 0..10_usize {
//!             stack.append(vec![i.pow(2), i.pow(3), i.pow(4)].into_iter().collect());
//!         }
//!     });
//! });
//!
//! stack.into_iter().for_each(|v| print!("{}, ", v));
//! ```
mod base;

pub use base::Stack;

extern crate alloc;

/// [Operation](Operation) is used for fuzzing purposes to provide randomized input.
#[cfg(feature = "arbitrary")]
#[derive(Clone, Debug)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub enum Operation<T> {
    Push { item: T },
    Pop,
    PopPush,
    Append { items: Vec<T> },
    Peek,
}

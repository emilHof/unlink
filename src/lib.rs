mod base;
mod tagged;

pub use base::Stack;
pub(crate) use tagged::MaybeTagged;

extern crate alloc;

#[cfg(feature = "arbitrary")]
#[derive(Clone, Debug)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub enum Operation<T> {
    Push { item: T },
    Pop,
    PopPush,
    Ext { items: Vec<T> },
    Iter,
}

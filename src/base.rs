use crate::MaybeTagged;
use alloc::alloc::{alloc, dealloc};
use core::ptr::{null_mut, NonNull};
use core::sync::atomic::{AtomicUsize, Ordering};
use haphazard::{Domain, HazardPointer, Singleton};

pub struct Node<V> {
    pub val: V,
    pub(crate) next: MaybeTagged<Self>,
}

impl<V> Node<V> {
    fn new(val: V) -> *mut Self {
        unsafe {
            let node = Self::alloc();
            core::ptr::write(&mut (*node).val, val);
            core::ptr::write_bytes(&mut (*node).next, 0, 0);
            node
        }
    }

    unsafe fn alloc() -> *mut Self {
        let layout = layout::<Self>();
        alloc(layout).cast::<Self>()
    }

    unsafe fn dealloc(raw: *mut Self) {
        let layout = layout::<Self>();
        dealloc(raw.cast(), layout);
    }

    unsafe fn drop(raw: *mut Self) {
        core::ptr::drop_in_place(&mut (*raw).val);
        Self::dealloc(raw);
    }
}

const unsafe fn layout<T>() -> core::alloc::Layout {
    let size = core::mem::size_of::<T>();
    let align = core::mem::align_of::<T>();
    core::alloc::Layout::from_size_align_unchecked(size, align)
}

pub struct Stack<V> {
    head: MaybeTagged<Node<V>>,
    len: AtomicUsize,
}

struct UniqueFamily;

unsafe impl Singleton for UniqueFamily {}

static UNIQUE_FAMILY: Domain<UniqueFamily> = Domain::new(&UniqueFamily);

impl<V> Stack<V> {
    pub fn new() -> Self {
        Stack {
            head: MaybeTagged::new(null_mut()),
            len: AtomicUsize::new(0),
        }
    }

    pub fn len(&self) -> usize {
        let len = self.len.load(std::sync::atomic::Ordering::Relaxed);
        if len > isize::MAX as usize {
            0
        } else {
            len
        }
    }
}

impl<V> Stack<V>
where
    V: Send + Sync,
{
    pub fn push(&self, val: V) {
        let node_ptr = Node::new(val);
        let node = NodeRef::from_ptr(node_ptr);

        let mut head_ptr = self.head.load_ptr();

        node.next.store_ptr(head_ptr);

        while let Err((now, _)) =
            self.head
                .compare_exchange(head_ptr, node_ptr, Ordering::AcqRel, Ordering::Relaxed)
        {
            node.next.store_ptr(now);
            head_ptr = now;
        }

        self.len.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn pop(&self) -> Option<Entry<'_, V>> {
        let mut old_head = NodeRef::from_maybe_tagged(&self.head)?;

        let mut next_ptr = old_head.next.load_ptr();

        while let Err((_, _)) = self.head.compare_exchange(
            old_head.as_ptr(),
            next_ptr,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            old_head = NodeRef::from_maybe_tagged(&self.head)?;

            next_ptr = old_head.next.load_ptr();
        }

        unsafe {
            UNIQUE_FAMILY.retire_ptr::<_, DropNode<_>>(old_head.as_ptr());
        }

        Some(old_head.into())
    }

    pub fn peek(&self) -> Option<NodeRef<'_, V>> {
        NodeRef::from_maybe_tagged(&self.head)
    }

    pub fn extend(&self, other: Self) {
        let Some(new_head) = NodeRef::from_maybe_tagged(&other.head) else {
            return;
        };

        other.head.store_ptr(null_mut());

        let mut tail = new_head.as_ptr();
        unsafe {
            while !(*tail).next.load_ptr().is_null() {
                tail = (*tail).next.load_ptr();
            }
            tail
        };

        let mut old_head = self.head.load_ptr();

        unsafe {
            (*tail).next.store_ptr(old_head);
        }

        while let Err((now, _)) = self.head.compare_exchange(
            old_head,
            new_head.as_ptr(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            old_head = now;
            unsafe {
                (*tail).next.store_ptr(old_head);
            }
        }
    }

    pub fn iter<'a>(&'a self) -> Iter<'a, V> {
        Iter { next: self.peek() }
    }
}

impl<V> Drop for Stack<V> {
    fn drop(&mut self) {
        let mut curr = self.head.load_ptr();

        unsafe {
            while !curr.is_null() {
                let next = (*curr).next.load_ptr();
                Node::drop(curr);
                curr = next;
            }
        }
    }
}

pub struct NodeRef<'a, V> {
    node: NonNull<Node<V>>,
    _hazard: HazardPointer<'a, UniqueFamily>,
}

impl<'a, V> NodeRef<'a, V> {
    fn as_ptr(&self) -> *mut Node<V> {
        self.node.as_ptr()
    }
}

impl<'a, V> core::ops::Deref for NodeRef<'a, V> {
    type Target = Node<V>;
    fn deref(&self) -> &Self::Target {
        unsafe { self.node.as_ref() }
    }
}

impl<'a, V> NodeRef<'a, V> {
    pub(crate) fn from_ptr(ptr: *mut Node<V>) -> Self {
        assert!(!ptr.is_null());

        let mut _hazard = HazardPointer::new_in_domain(&UNIQUE_FAMILY);

        _hazard.protect_raw(ptr);

        let node = unsafe { NonNull::new_unchecked(ptr) };

        NodeRef { node, _hazard }
    }

    fn from_maybe_tagged(maybe_tagged: &MaybeTagged<Node<V>>) -> Option<Self> {
        let mut _hazard = HazardPointer::new_in_domain(&UNIQUE_FAMILY);
        let mut ptr = maybe_tagged.load_ptr();

        _hazard.protect_raw(ptr);

        let mut v_ptr = maybe_tagged.load_ptr();

        while !core::ptr::eq(ptr, v_ptr) {
            ptr = v_ptr;
            _hazard.protect_raw(ptr);

            v_ptr = maybe_tagged.load_ptr();
        }

        if ptr.is_null() {
            None
        } else {
            unsafe {
                Some(NodeRef {
                    node: core::ptr::NonNull::new_unchecked(ptr),
                    _hazard,
                })
            }
        }
    }
}

#[repr(transparent)]
struct DropNode<V>(NonNull<Node<V>>);

impl<V> Drop for DropNode<V> {
    fn drop(&mut self) {
        unsafe {
            Node::drop(self.0.as_ptr());
        }
    }
}

impl<V> core::ops::Deref for DropNode<V> {
    type Target = Node<V>;
    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref() }
    }
}

unsafe impl<V> haphazard::raw::Pointer<Node<V>> for DropNode<V> {
    fn into_raw(self) -> *mut Node<V> {
        self.0.as_ptr()
    }

    unsafe fn from_raw(ptr: *mut Node<V>) -> Self {
        Self(NonNull::new_unchecked(ptr))
    }
}

pub struct Entry<'a, V> {
    node: NonNull<Node<V>>,
    _hazard: haphazard::HazardPointer<'a, UniqueFamily>,
}

impl<'a, V> core::ops::Deref for Entry<'a, V> {
    type Target = Node<V>;
    fn deref(&self) -> &Self::Target {
        unsafe { self.node.as_ref() }
    }
}

impl<'a, V> core::ops::Drop for Entry<'a, V> {
    fn drop(&mut self) {
        UNIQUE_FAMILY.eager_reclaim();
    }
}

impl<'a, V> From<NodeRef<'a, V>> for Entry<'a, V> {
    fn from(node_ref: NodeRef<'a, V>) -> Self {
        unsafe { core::mem::transmute(node_ref) }
    }
}

pub struct Iter<'a, V> {
    next: Option<NodeRef<'a, V>>,
}

impl<'a, V> Iterator for Iter<'a, V> {
    type Item = NodeRef<'a, V>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(next) = self.next.take() {
            self.next = NodeRef::from_maybe_tagged(&next.next);
            return Some(next);
        }

        None
    }
}

mod test {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_new_node() {
        let node = Node::new(1);

        unsafe {
            Node::dealloc(node);
        }
    }

    #[test]
    fn test_push_front() {
        let list = Stack::new();

        list.push(1);

        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);

        assert_eq!(list.len(), 1);
    }

    #[test]
    fn test_push_pop_sync() {
        let list = Arc::new(Stack::new());

        let mut threads = vec![];

        for i in 0..10 {
            let list = list.clone();

            threads.push(std::thread::spawn(move || {
                for j in 0..100 {
                    if rand::random::<u8>() % 3 != 0 {
                        list.push(i);
                    } else {
                        list.pop();
                    }
                }
            }))
        }

        for thead in threads {
            thead.join().unwrap();
        }

        list.iter().for_each(|n| println!("val: {}", n.val));
    }

    #[test]
    fn test_extend() {
        let expected = vec![2, 3, 7, 2, 0, 0, 3, 4, 2, 5];

        let stack = Stack::new();

        expected[expected.len() / 2..]
            .iter()
            .rev()
            .for_each(|&e| stack.push(e));

        let other = Stack::new();

        expected[..expected.len() / 2]
            .iter()
            .rev()
            .for_each(|&e| other.push(e));

        stack.extend(other);

        let actual: Vec<i32> = stack.iter().map(|e| e.val).collect();

        assert_eq!(expected, actual);
    }
}

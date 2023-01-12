use alloc::alloc::{alloc, dealloc};
use core::ptr::{null_mut, NonNull};
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use haphazard::{Domain, HazardPointer, Singleton};

struct Node<V> {
    pub val: V,
    next: AtomicPtr<Self>,
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

/// [UniqueFamily](UniqueFamily) enables type checking for [HazardPointers](HazardPointer)
struct UniqueFamily;

unsafe impl Singleton for UniqueFamily {}

static UNIQUE_FAMILY: Domain<UniqueFamily> = Domain::new(&UniqueFamily);

pub struct Stack<V> {
    head: AtomicPtr<Node<V>>,
    domain: &'static Domain<UniqueFamily>,
    len: AtomicUsize,
}

impl<V> core::fmt::Debug for Stack<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stack").finish()
    }
}

impl<V> Stack<V> {
    pub fn new() -> Self {
        Stack {
            head: AtomicPtr::new(null_mut()),
            domain: &UNIQUE_FAMILY,
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

        let mut head_ptr = self.head.load(Ordering::SeqCst);

        node.next.store(head_ptr, Ordering::SeqCst);

        while let Err(now) =
            self.head
                .compare_exchange(head_ptr, node_ptr, Ordering::AcqRel, Ordering::Relaxed)
        {
            node.next.store(now, Ordering::SeqCst);
            head_ptr = now;
        }

        self.len.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn pop(&self) -> Option<Entry<'_, V>> {
        let mut old_head = NodeRef::from_atomic_ptr(&self.head)?;

        let mut next_ptr = old_head.next.load(Ordering::SeqCst);

        while let Err(_) = self.head.compare_exchange(
            old_head.as_ptr(),
            next_ptr,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            old_head = NodeRef::from_atomic_ptr(&self.head)?;

            next_ptr = old_head.next.load(Ordering::SeqCst);
        }

        unsafe {
            self.domain.retire_ptr::<_, DropNode<_>>(old_head.as_ptr());
            self.domain.eager_reclaim();
        }

        Some(old_head.into())
    }

    pub fn peek(&self) -> Option<Entry<'_, V>> {
        NodeRef::from_atomic_ptr(&self.head).map(|n| n.into())
    }

    pub fn extend(&self, other: Self) {
        let Some(new_head) = NodeRef::from_atomic_ptr(&other.head) else {
            return;
        };

        other.head.store(null_mut(), Ordering::SeqCst);

        let mut tail = new_head.as_ptr();

        unsafe {
            while !(*tail).next.load(Ordering::SeqCst).is_null() {
                tail = (*tail).next.load(Ordering::SeqCst);
            }
            tail
        };

        let mut old_head = self.head.load(Ordering::SeqCst);

        unsafe {
            (*tail).next.store(old_head, Ordering::SeqCst);
        }

        while let Err(head_now) = self.head.compare_exchange(
            old_head,
            new_head.as_ptr(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            old_head = head_now;
            unsafe {
                (*tail).next.store(old_head, Ordering::SeqCst);
            }
        }
    }
}

impl<V> Drop for Stack<V> {
    fn drop(&mut self) {
        // Deallocate all pointers that are no longer referred to.
        self.domain.eager_reclaim();

        let mut curr = self.head.load(Ordering::SeqCst);

        // # Safety: We have exclusive ownership of self.
        unsafe {
            while !curr.is_null() {
                let next = (*curr).next.load(Ordering::SeqCst);
                Node::drop(curr);
                curr = next;
            }
        }
    }
}

/// [NodeRef](NodeRef) is a protected `*mut` to a Node. It will be valid until it is dropped.
struct NodeRef<'a, V> {
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

    fn from_atomic_ptr(ptr: &AtomicPtr<Node<V>>) -> Option<Self> {
        let mut _hazard = HazardPointer::new_in_domain(&UNIQUE_FAMILY);

        let node = _hazard.protect_ptr(&ptr)?.0;

        Some(NodeRef { node, _hazard })
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
    type Target = V;
    fn deref(&self) -> &Self::Target {
        unsafe { &self.node.as_ref().val }
    }
}

impl<'a, V> From<NodeRef<'a, V>> for Entry<'a, V> {
    fn from(node_ref: NodeRef<'a, V>) -> Self {
        unsafe { core::mem::transmute(node_ref) }
    }
}

pub struct IntoIter<V> {
    stack: Stack<V>,
}

impl<V> Iterator for IntoIter<V> {
    type Item = V;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.stack.head.load(Ordering::Acquire);
        if next.is_null() {
            return None;
        }

        unsafe {
            self.stack
                .head
                .store((*next).next.load(Ordering::Acquire), Ordering::Release);

            let val = core::ptr::read(&(*next).val);

            Node::<V>::dealloc(next);

            Some(val)
        }
    }
}

impl<V> IntoIterator for Stack<V> {
    type Item = V;
    type IntoIter = IntoIter<V>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter { stack: self }
    }
}

impl<V> FromIterator<V> for Stack<V>
where
    V: Send + Sync,
{
    fn from_iter<T: IntoIterator<Item = V>>(iter: T) -> Self {
        let stack = Stack::new();
        for val in iter {
            stack.push(val);
        }

        stack
    }
}

mod test {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[derive(Debug)]
    struct CountOnDrop<V> {
        val: V,
        counter: Arc<AtomicUsize>,
    }

    impl<V> Drop for CountOnDrop<V> {
        fn drop(&mut self) {
            self.counter.fetch_add(1, Ordering::SeqCst);
        }
    }

    unsafe impl<V> Send for CountOnDrop<V> {}
    unsafe impl<V> Sync for CountOnDrop<V> {}

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
                for _ in 0..100 {
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

        Arc::try_unwrap(list)
            .unwrap()
            .into_iter()
            .for_each(|e| println!("{}", e));
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

        let actual: Vec<i32> = stack.into_iter().map(|e| e).collect();

        assert_eq!(expected, actual);
    }

    #[test]
    fn test_hazard() {
        let stack = Stack::new();
        let counter = Arc::new(AtomicUsize::new(0));

        stack.extend(
            vec![
                CountOnDrop {
                    counter: counter.clone(),
                    val: 0,
                },
                CountOnDrop {
                    counter: counter.clone(),
                    val: 2,
                },
                CountOnDrop {
                    counter: counter.clone(),
                    val: 3,
                },
            ]
            .into_iter()
            .fold(Stack::new(), |stack, e| {
                stack.push(e);
                stack
            }),
        );

        let top = stack.peek().unwrap();

        let owned = stack.pop().unwrap();

        assert_eq!(top.val, owned.val);

        drop(owned);

        assert_eq!(counter.load(Ordering::SeqCst), 0);

        assert!(top.val != -1);

        drop(top);

        assert_eq!(counter.load(Ordering::SeqCst), 0);

        stack.domain.eager_reclaim();

        assert_eq!(counter.load(Ordering::SeqCst), 1);

        stack.pop();

        assert_eq!(counter.load(Ordering::SeqCst), 1);

        stack.pop();

        assert_eq!(counter.load(Ordering::SeqCst), 2);

        stack.push(CountOnDrop {
            val: 0,
            counter: counter.clone(),
        });

        assert_eq!(counter.load(Ordering::SeqCst), 2);

        stack.pop();

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }
}

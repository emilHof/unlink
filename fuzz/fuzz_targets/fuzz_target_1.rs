#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::Arc;
use unlink::{Operation, Stack};

fuzz_target!(|ops: Vec<unlink::Operation<i32>>| {
    let stack = Arc::new(Stack::new());

    let mut threads = vec![];

    let len = ops.len();

    for sub_ops in ops.chunks(std::cmp::max(len / 20, 1)) {
        let sub_ops = sub_ops.iter().map(|op| op.clone()).collect::<Vec<_>>();
        let stack = stack.clone();

        threads.push(std::thread::spawn(move || {
            sub_ops.into_iter().for_each(|op| match op {
                Operation::Pop => {
                    stack.pop();
                }
                Operation::PopPush => {
                    if let Some(e) = stack.pop() {
                        stack.push(e.val * e.val)
                    }
                }
                Operation::Push { item } => stack.push(item),
                Operation::Ext { items } => {
                    let other = Stack::new();
                    items.into_iter().for_each(|item| other.push(item));
                    stack.extend(other);
                }
                Operation::Iter => {
                    stack.iter().find(|item| item.val % 10214 == 0);
                }
            })
        }))
    }

    for thread in threads {
        thread.join().unwrap()
    }
    // fuzzed code goes here
});

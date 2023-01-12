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
        let stack: Arc<Stack<Arc<i32>>> = stack.clone();

        threads.push(std::thread::spawn(move || {
            sub_ops.into_iter().for_each(|op| match op {
                Operation::Peek => {
                    if let Some(e) = stack.peek() {
                        std::thread::sleep(std::time::Duration::from_nano(10));
                        stack.push(Arc::new(**e));
                    }
                }
                Operation::Pop => {
                    stack.pop();
                }
                Operation::PopPush => {
                    if let Some(e) = stack.pop() {
                        stack.push(Arc::new(**e * **e))
                    }
                }
                Operation::Push { item } => stack.push(Arc::new(item)),
                Operation::Ext { items } => {
                    let other = Stack::new();
                    items
                        .into_iter()
                        .for_each(|item| other.push(Arc::new(item)));
                    stack.extend(other);
                }
            })
        }))
    }

    for thread in threads {
        thread.join().unwrap()
    }
    // fuzzed code goes here
});

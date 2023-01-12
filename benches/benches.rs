#![feature(test)]
extern crate test;

use lockfree::stack as lockfree;
use std::sync::Arc;

#[bench]
fn bench_push_pop_unlink(b: &mut test::Bencher) {
    b.iter(|| {
        let list = Arc::new(unlink::Stack::new());

        let mut threads = vec![];

        for i in 0..20 {
            let list = list.clone();

            threads.push(std::thread::spawn(move || {
                for _ in 0..10_000 {
                    if rand::random::<u8>() % 2 != 0 {
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
    })
}

#[bench]
fn bench_push_pop_lockfree(b: &mut test::Bencher) {
    b.iter(|| {
        let list = Arc::new(lockfree::Stack::new());

        let mut threads = vec![];

        for i in 0..20 {
            let list = list.clone();

            threads.push(std::thread::spawn(move || {
                for _ in 0..10_000 {
                    if rand::random::<u8>() % 2 != 0 {
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
    })
}

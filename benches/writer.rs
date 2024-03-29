#![feature(test)]
#![allow(non_snake_case, unused_variables)]

extern crate test;
extern crate thread_io;

use std::io::{sink, Write};

fn get_data(len: usize) -> Vec<u8> {
    b"The quick brown fox jumps over the lazy dog"
        .iter()
        .cycle()
        .take(len)
        .cloned()
        .collect()
}

static DATA_LEN: usize = 1 << 26;
static CHUNK_SIZE: usize = 1024;

macro_rules! bench {
    ($name:ident, $bufsize:expr, $queuelen:expr) => {
        #[bench]
        fn $name(b: &mut test::Bencher) {
            let data = get_data(DATA_LEN);
            b.bytes = data.len() as u64;
            let mut out = sink();
            b.iter(move || {
                thread_io::write::writer($bufsize, $queuelen, &mut out, |w| {
                    for chunk in data.chunks(CHUNK_SIZE) {
                        w.write_all(chunk)?;
                    }
                    Ok::<(), ::std::io::Error>(())
                })
                .unwrap();
            });
        }
    };
}

macro_rules! bench_native {
    ($name:ident, $bufsize:expr) => {
        #[bench]
        fn $name(b: &mut test::Bencher) {
            let data = get_data(DATA_LEN);
            b.bytes = data.len() as u64;
            let mut out = sink();
            b.iter(move || {
                for chunk in data.chunks(CHUNK_SIZE) {
                    out.write_all(chunk).expect("Write error");
                }
            });
        }
    };
}

bench!(write_thread_68k_3, 1024 * 68, 3);
bench!(write_thread_256k_3, 1 << 18, 3);
bench!(write_thread_512k_2, 1 << 19, 2);
bench!(write_thread_512k_3, 1 << 19, 3);
bench!(write_thread_512k_4, 1 << 19, 4);
bench!(write_thread_512k_5, 1 << 19, 5);
bench!(write_thread_1m_3, 1 << 20, 3);
bench!(write_thread_2m_3, 1 << 21, 3);
bench!(write_thread_4m_3, 1 << 22, 3);

bench_native!(write_native_256k, 1 << 18);
bench_native!(write_native_512k, 1 << 19);
bench_native!(write_native_1m, 1 << 20);

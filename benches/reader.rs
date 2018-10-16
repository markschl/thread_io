#![feature(test)]
#![allow(non_snake_case, unused_variables)]

extern crate test;
extern crate thread_io;

use std::io::{Read, Cursor, ErrorKind};


fn get_data(len: usize) -> Vec<u8> {
    b"The quick brown fox jumps over the lazy dog"
        .into_iter()
        .cycle()
        .take(len)
        .cloned()
        .collect()
}

static DATA_LEN: usize = 1 << 26;


macro_rules! bench {
    ($name:ident, $bufsize:expr, $queuelen:expr) => {
        #[bench]
        fn $name(b: &mut test::Bencher) {
            let data = get_data(DATA_LEN);
            b.bytes = data.len() as u64;
            b.iter(move || {
                let mut buf = vec![0; $bufsize];
                thread_io::read::reader($bufsize, $queuelen, Cursor::new(&data), |rdr| {
                    loop {
                        if let Err(e) = rdr.read_exact(&mut buf) {
                            if e.kind() == ErrorKind::UnexpectedEof {
                                break;
                            } else {
                                return Err(e);
                            }
                        }
                    }
                    Ok(())
                }).unwrap();

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
            b.iter(move || {
                let mut buf = vec![0; $bufsize];
                let mut rdr = Cursor::new(&data);
                loop {
                    if let Err(e) = rdr.read_exact(&mut buf) {
                        if e.kind() == ErrorKind::UnexpectedEof {
                            break;
                        } else {
                            panic!(e);
                        }
                    }
                }
            });
        }
    };
}



bench!(thread_32k_2, 1 << 15, 2);
bench!(thread_64k_2, 1 << 16, 2);
bench!(thread_128k_2, 1 << 17, 2);
bench!(thread_256k_2, 1 << 18, 2);
bench!(thread_512k_2, 1 << 19, 2);
bench!(thread_512k_3, 1 << 19, 3);
bench!(thread_512k_4, 1 << 19, 4);
bench!(thread_512k_5, 1 << 19, 5);
bench!(thread_1m_2, 1 << 20, 2);
bench!(thread_2m_2, 1 << 21, 2);

bench_native!(native_8k, 1 << 13);
bench_native!(native_64k, 1 << 16);
bench_native!(native_256k, 1 << 18);
bench_native!(native_1m, 1 << 20);

extern crate thread_io;

use thread_io::read::*;
use std::io::{Read,self};
use std::cmp::min;

struct Reader<'a> {
    data: &'a [u8],
    block_size: usize,
    // used to test what happens to errors that are
    // stuck in the queue
    fails_after: usize
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], block_size: usize, fails_after: usize) -> Reader {
        Reader {
            data: data,
            block_size: block_size,
            fails_after: fails_after
        }
    }
}

impl<'a> Read for Reader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.fails_after == 0 {
            return Err(io::Error::new(io::ErrorKind::Other, "read err"));
        }
        self.fails_after -= 1;
        let amt = min(self.data.len(), min(buf.len(), self.block_size));
        let (a, b) = self.data.split_at(amt);
        buf[..amt].copy_from_slice(a);
        self.data = b;
        Ok(amt)
    }
}

// Repeatedly reads from rdr into a buffer of size `chunksize`. The buffer contents are
// appended to the output until EOF occurs, and the output is returned.
fn read_chunks<R: io::Read>(mut rdr: R, chunksize: usize) -> io::Result<Vec<u8>> {
    let mut out = vec![];
    let mut buf = vec![0; chunksize];
    loop {
        let n = rdr.read(buf.as_mut_slice())?;
        out.extend_from_slice(&buf[..n]);
        if n == 0 {
            break;
        }
    }
    Ok(out)
}

#[test]
fn read() {
    let text = b"The quick brown fox";
    let len = text.len();

    for channel_bufsize in 1..len {
        for rdr_block_size in 1..len {
            for out_bufsize in 1..len {
                for queuelen in 1..len {
                    // test the mock reader itself
                    let mut rdr = Reader::new(text, rdr_block_size, ::std::usize::MAX);
                    assert_eq!(read_chunks(rdr, out_bufsize).unwrap().as_slice(), &text[..]);

                    // test threaded reader
                    let mut rdr = Reader::new(text, rdr_block_size, ::std::usize::MAX);
                    let out = reader(channel_bufsize, queuelen, rdr, |r| read_chunks(r, out_bufsize)).unwrap();

                    if out.as_slice() != &text[..] {
                        panic!(format!(
                            "left != right at channel bufsize: {}, reader bufsize: {}, final reader bufsize {}, queue length: {}\nleft:  {:?}\nright: {:?}",
                            channel_bufsize, rdr_block_size, out_bufsize, queuelen, &out, &text[..]
                        ));
                    }
                }
            }
        }
    }
}


#[test]
fn read_fail() {
    let text = b"The quick brown fox";
    let len = text.len();

    for channel_bufsize in 1..len {
        for queuelen in 1..len {
            let mut out = vec![0];
            let mut rdr = Reader::new(text, channel_bufsize, len / channel_bufsize);
            let res: io::Result<_> = reader(channel_bufsize, queuelen, rdr, |r| {
                while r.read(&mut out)? > 0 {
                }
                Ok(())
            });

            if let Err(e) = res {
                assert_eq!(&format!("{}", e), "read err");
            } else {
                panic!(format!(
                    "read should fail at bufsize: {}, queue length: {}",
                    channel_bufsize, queuelen
                ));
            }
        }
    }
}

#[test]
fn reader_init_fail() {
    let e = io::Error::new(io::ErrorKind::Other, "init err");
    let res = reader_init(5, 2, || Err::<&[u8], _>(e), |_| {Ok(())});
    if let Err(e) = res {
        assert_eq!(&format!("{}", e), "init err");
    } else {
        panic!("init should fail");
    }
}

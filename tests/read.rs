use std::cmp::min;
use std::io::{self, Read};
use thread_io::read::*;

struct Reader<'a> {
    data: &'a [u8],
    block_size: usize,
    // used to test what happens to errors that are
    // stuck in the queue
    fails_after: usize,
    interrups_after: usize,
    panic: bool,
}

impl<'a> Reader<'a> {
    fn new(
        data: &'a [u8],
        block_size: usize,
        fails_after: Option<usize>,
        interrups_after: Option<usize>,
        panic: bool,
    ) -> Reader {
        Reader {
            data,
            block_size,
            fails_after: fails_after.unwrap_or(usize::max_value()),
            interrups_after: interrups_after.unwrap_or(usize::max_value()),
            panic,
        }
    }
}

impl<'a> Read for Reader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.fails_after == 0 {
            if self.panic {
                panic!("read err");
            } else {
                return Err(io::Error::new(io::ErrorKind::Other, "read err"));
            }
        }
        if self.interrups_after == 0 {
            self.interrups_after = usize::max_value();
            return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"));
        }
        self.fails_after -= 1;
        self.interrups_after -= 1;
        let amt = min(self.data.len(), min(buf.len(), self.block_size));
        let (a, b) = self.data.split_at(amt);
        buf[..amt].copy_from_slice(a);
        self.data = b;
        Ok(amt)
    }
}

// Repeatedly reads from rdr into a buffer of size `chunksize`. The buffer contents are
// appended to the output until EOF occurs, and the output is returned.
fn read_chunks<R: io::Read>(mut rdr: R, chunksize: usize, resume: bool) -> io::Result<Vec<u8>> {
    let mut out = vec![];
    let mut buf = vec![0; chunksize];
    loop {
        match rdr.read(buf.as_mut_slice()) {
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                if n == 0 {
                    break;
                }
            }
            Err(e) => {
                if resume && e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
        }
    }
    Ok(out)
}

#[test]
fn read() {
    let text = b"The quick brown fox";
    let n = text.len() + 3;

    for channel_bufsize in (1..n).step_by(2) {
        for rdr_block_size in (1..n).step_by(2) {
            for out_bufsize in (1..n).step_by(2) {
                for queuelen in (1..n).step_by(2) {
                    // test the mock reader itself
                    let rdr = Reader::new(text, rdr_block_size, None, None, false);
                    assert_eq!(
                        read_chunks(rdr, out_bufsize, false).unwrap().as_slice(),
                        &text[..]
                    );

                    // test threaded reader
                    let rdr = Reader::new(text, rdr_block_size, None, None, false);
                    let out = reader(channel_bufsize, queuelen, rdr, |r| {
                        read_chunks(r, out_bufsize, false)
                    })
                    .unwrap();

                    if out.as_slice() != &text[..] {
                        panic!(
                            "left != right at channel bufsize: {}, reader bufsize: {}, final reader bufsize {}, queue length: {}\nleft:  {:?}\nright: {:?}",
                            channel_bufsize, rdr_block_size, out_bufsize, queuelen, &out, &text[..]
                        );
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
            let rdr = Reader::new(
                text,
                channel_bufsize,
                Some(len / channel_bufsize),
                None,
                false,
            );
            let res: io::Result<_> = reader(channel_bufsize, queuelen, rdr, |r| {
                while r.read(&mut out)? > 0 {}
                Ok(())
            });

            if let Err(e) = res {
                assert_eq!(&format!("{}", e), "read err");
            } else {
                panic!(
                    "read should fail at bufsize: {}, queue length: {}",
                    channel_bufsize, queuelen
                );
            }
        }
    }
}

#[test]
#[should_panic(expected = "read err")]
fn read_panic() {
    let text = b"The quick brown fox";
    let rdr = Reader::new(text, 1, Some(1), None, true);
    let _res: io::Result<_> = reader(1, 1, rdr, |r| {
        r.read_to_end(&mut Vec::new())?;
        Ok(())
    });
}

#[test]
fn read_fail_processing() {
    let text = b"The quick brown fox";

    let rdr = Reader::new(text, 1, Some(1), None, false);
    let res: Result<(), &'static str> = reader(1, 1, rdr, |_r| Err("gave up"));

    if let Err(e) = res {
        assert_eq!(&format!("{}", e), "gave up");
    } else {
        panic!("read should fail");
    }
}

#[test]
fn read_interrupted() {
    let text = b"The quick brown fox";
    let rdr = Reader::new(text, 2, None, Some(1), false);
    assert_eq!(read_chunks(rdr, 3, true).unwrap().as_slice(), &text[..]);
}

#[test]
#[should_panic(expected = "gave up")]
fn read_panic_processing() {
    let text = b"The quick brown fox";

    let rdr = Reader::new(text, 1, Some(1), None, false);
    let _res: Result<(), &'static str> = reader(1, 1, rdr, |_r| panic!("gave up"));
}

#[test]
fn reader_init_fail() {
    let e = io::Error::new(io::ErrorKind::Other, "init err");
    let res = reader_init(
        5,
        2,
        || Err::<&[u8], _>(e),
        |reader| {
            reader.read_to_end(&mut Vec::new())?;
            Ok(())
        },
    );
    if let Err(e) = res {
        assert_eq!(&format!("{}", e), "init err");
    } else {
        panic!("init should fail");
    }
}

#[test]
#[should_panic(expected = "init panic")]
fn reader_init_panic() {
    reader_init::<&[u8], _, _, _, _>(5, 2, || panic!("init panic"), |_| Ok::<_, ()>(())).unwrap();
}

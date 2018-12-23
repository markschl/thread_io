#![no_main]
#[macro_use] extern crate libfuzzer_sys;
extern crate thread_io;

use std::io::{self, Read};
use std::cmp::{min, max};

// runs the thread_io::read::reader using different buffer sizes and queue lengths
// using a mock reader (as in tests::read, but more variables are changed randomly) to ensure that
// the returned data will always be the same
fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }
    //println!("{:?}", data);
    let (channel_bufsize, data) = data.split_first().unwrap();
    let channel_bufsize = max(1, *channel_bufsize as usize / 4);
    let (queuelen, data) = data.split_first().unwrap();
    let queuelen = max(1, *queuelen as usize / 4);
    // size of buffer we are reading into in main thread
    let (out_bufsize, mut data) = data.split_first().unwrap();
    let out_bufsize = max(1, *out_bufsize as usize / 4);

    // determine sizes of chunks being read by mock reader wrapped in thread
    let mut cum_sizes = 0;
    let mut inner_chunk_sizes = vec![];
    while let Some((inner_chunk_size, _data)) = data.split_first() {
        cum_sizes += *inner_chunk_size as usize;
        data = _data;
        if cum_sizes > data.len() {
            break;
        }
        inner_chunk_sizes.push(inner_chunk_size);
    }

    if data.len() == 0 {
        return;
    }

    //println!("queue len: {}, channel_bufsize: {}, out_bufsize: {}, inner_chunk_sizes: {:?}, data len: {} data {:?}", queuelen, channel_bufsize, out_bufsize, inner_chunk_sizes, data.len(), data);

    // test the mock reader itself
    let c = inner_chunk_sizes.iter().map(|&&c| c as usize);
    let rdr = Reader::new(data, c, None);
    assert_eq!(read_chunks(rdr, out_bufsize).unwrap().as_slice(), &data[..]);

    // test threaded reader
    let c = inner_chunk_sizes.iter().map(|&&c| c as usize);
    let rdr = Reader::new(data, c, None);
    let out = thread_io::read::reader(
        channel_bufsize,
        queuelen,
        rdr,
        |r| read_chunks(r, out_bufsize)
    ).unwrap();

    assert_eq!(out.as_slice(), &data[..]);

});


struct Reader<'a, C: Iterator<Item=usize>> {
    data: &'a [u8],
    chunk_sizes: C,
    fails_after: Option<usize>
}

impl<'a, C: Iterator<Item=usize>> Reader<'a, C> {
    fn new(data: &'a [u8], chunk_sizes: C, fails_after: Option<usize>) -> Self {
        Reader {
            data: data,
            chunk_sizes: chunk_sizes,
            fails_after: fails_after,
        }
    }
}

impl<'a, C: Iterator<Item=usize>> Read for Reader<'a, C> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(f) = self.fails_after.as_mut() {
            if *f == 0 {
                return Err(io::Error::new(io::ErrorKind::Other, "expected fail"));
            }
            *f -= 1;
        }
        let chunk_size = self.chunk_sizes.next();
        if chunk_size == Some(0) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "interrupted"));
        }
        let chunk_size = chunk_size.unwrap_or_else(|| max(self.data.len(), 0));
        let amt = min(self.data.len(), min(buf.len(), chunk_size));
        let (a, b) = self.data.split_at(amt);
        buf[..amt].copy_from_slice(a);
        self.data = b;
        Ok(amt)
    }
}

// Repeatedly reads from rdr into a buffer of size `chunksize`. The buffer contents are
// appended to the output until EOF occurs, and the output is returned.
fn read_chunks<R: io::Read>(mut rdr: R, out_buf_size: usize) -> io::Result<Vec<u8>> {
    let mut out = vec![];
    let mut buf = vec![0; out_buf_size];
    loop {
        let res = rdr.read(buf.as_mut_slice());
        let n = match res {
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        out.extend_from_slice(&buf[..n]);
        if n == 0 {
            break;
        }
    }
    Ok(out)
}

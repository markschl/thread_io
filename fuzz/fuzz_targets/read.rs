#![no_main]

use std::io::{self, Read};
use std::cmp::{min, max};
use libfuzzer_sys::fuzz_target;
use libfuzzer_sys::arbitrary::Arbitrary;

#[derive(Arbitrary, Debug)]
pub struct Config {
    channel_bufsize: u16,
    queuelen: u8,
    out_bufsize: u16,
    data_len: u16,
    // The list of reads to be done:
    // - 0 means ErrorKind::Interrupted
    // - > 0 means n bytes will be read into target buffer
    chunk_sizes: Vec<u8>,
}

// runs the thread_io::read::reader using different buffer sizes and queue lengths
// using a mock reader (as in tests::read, but more variables are changed randomly) to ensure that
// the returned data will always be the same
fuzz_target!(|cfg: Config| {
    //println!("{:?}", cfg);
    let mut cfg = cfg;
    cfg.channel_bufsize = max(1, cfg.channel_bufsize);
    cfg.queuelen = max(1, cfg.queuelen);
    cfg.out_bufsize = max(1, cfg.out_bufsize);

    if cfg.chunk_sizes.len() == 0 {
        return;
    }

    let data: Vec<u8> = (0..cfg.data_len).map(|_| 0).collect();

    // test the mock reader itself
    let c = cfg.chunk_sizes.iter().map(|&c| c as usize);
    let rdr = Reader::new(&data, c);
    assert_eq!(read_chunks(rdr, cfg.out_bufsize as usize).unwrap(), data);

    // test threaded reader
    let c = cfg.chunk_sizes.iter().map(|&c| c as usize);
    let rdr = Reader::new(&data, c);
    let out = thread_io::read::reader(
        cfg.channel_bufsize as usize,
        cfg.queuelen as usize,
        rdr,
        |r| read_chunks(r, cfg.out_bufsize as usize)
    ).unwrap();

    assert_eq!(out, data);

});


struct Reader<'a, C: Iterator<Item=usize>> {
    data: &'a [u8],
    chunk_sizes: C,
}

impl<'a, C: Iterator<Item=usize>> Reader<'a, C> {
    fn new(data: &'a [u8], chunk_sizes: C) -> Self {
        Reader {
            data: data,
            chunk_sizes: chunk_sizes,
        }
    }
}

impl<'a, C: Iterator<Item=usize>> Read for Reader<'a, C> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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

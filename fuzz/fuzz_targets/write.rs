#![no_main]

use std::io::{self, Write};
use std::cmp::{min, max};
use libfuzzer_sys::fuzz_target;
use libfuzzer_sys::arbitrary::Arbitrary;

#[derive(Arbitrary, Debug)]
pub struct Config {
    channel_bufsize: u16,
    queuelen: u8,
    writer_bufsize: u16,
    data: Vec<u8>,
}

// runs the thread_io::read::reader using different buffer sizes and queue lengths
// using a mock reader (as in tests::read, but more variables are changed randomly) to ensure that
// the returned cfg.data will always be the same
fuzz_target!(|cfg: Config| {
    //println!("{:?}", cfg);
    let mut cfg = cfg;
    cfg.channel_bufsize = max(1, cfg.channel_bufsize);
    cfg.queuelen = max(1, cfg.queuelen);
    cfg.writer_bufsize = max(1, cfg.writer_bufsize);

    // Test writer
    let w = thread_io::write::writer_finish(
        cfg.channel_bufsize as usize, 
        cfg.queuelen as usize,
        Writer::new(false, false, cfg.writer_bufsize as usize),
        |w| w.write(&cfg.data),
        |w| w
    ).unwrap().1;
    assert_eq!(w.data(), cfg.data.as_slice());

    // Test case in which write fails
    let w = Writer::new(true, false, cfg.writer_bufsize as usize);
    let res = thread_io::write::writer(
        cfg.channel_bufsize as usize, 
        cfg.queuelen as usize, 
        w, 
        |w| w.write(&cfg.data)
    );
    if let Err(e) = res {
        assert_eq!(&format!("{}", e), "write err");
    } else {
        panic!("write should fail");
    }

    // Test case in which flushing fails
    let w = Writer::new(false, true, cfg.writer_bufsize as usize);
    let res = thread_io::write::writer(
        cfg.channel_bufsize as usize, 
        cfg.queuelen as usize, 
        w, 
        |w| w.flush()
    );
    if let Err(e) = res {
        assert_eq!(&format!("{}", e), "flush err");
    } else {
        panic!("flush should fail");
    }
});

/// a writer that only writes its cfg.data to `Writer::cfg.data` upon `flush()`
#[derive(Clone)]
struct Writer {
    cache: Vec<u8>,
    data: Vec<u8>,
    write_fails: bool,
    flush_fails: bool,
    bufsize: usize,
}

impl Writer {
    fn new(write_fails: bool, flush_fails: bool, bufsize: usize) -> Writer {
        Writer {
            cache: vec![],
            data: vec![],
            write_fails: write_fails,
            flush_fails: flush_fails,
            bufsize: bufsize,
        }
    }

    fn data(&self) -> &[u8] {
        &self.data
    }
}

impl Write for Writer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.write_fails {
            return Err(io::Error::new(io::ErrorKind::Other, "write err"))
        }
        self.cache.write(&buffer[..min(buffer.len(), self.bufsize)])
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.flush_fails {
            Err(io::Error::new(io::ErrorKind::Other, "flush err"))
        } else {
            self.data.extend_from_slice(&self.cache);
            self.cache.clear();
            Ok(())
        }
    }
}

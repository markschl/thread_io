extern crate thread_io;

use thread_io::write::*;
use std::io::{Write,self};
use std::cmp::min;

/// a writer that only writes its data to `Writer::data` upon `flush()`
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

#[test]
fn write_thread() {
    let text = b"The quick brown fox jumps over the lazy dog";
    let len = text.len();

    for channel_bufsize in 1..len {
        for writer_bufsize in 1..len {
            for queuelen in 1..len {
                // Test the writer: write without flushing, which should result in empty output
                let mut w = writer_init_finish(channel_bufsize, queuelen,
                    || Ok(Writer::new(false, false, writer_bufsize)),
                    |w| w.write(text),
                    |w| w
                ).unwrap().1;
                assert_eq!(w.data(), b"");

                // Write with flushing: the output should be equal to the written data
                let mut w = writer_init_finish(channel_bufsize, queuelen,
                    || Ok(Writer::new(false, false, writer_bufsize)),
                    |w| w.write(text),
                    |mut w| {
                        w.flush().unwrap();
                        w
                    }).unwrap().1;
                if w.data() != &text[..] {
                    panic!(format!(
                        "write test failed: {:?} != {:?} at channel buffer size {}, writer bufsize {}, queue length {}",
                        String::from_utf8_lossy(w.data()), String::from_utf8_lossy(&text[..]),
                        channel_bufsize, writer_bufsize, queuelen
                    ));
                }

                w.flush().unwrap();
                if w.data() != &text[..] {
                    panic!(format!(
                        "write test failed: {:?} != {:?} at channel buffer size {}, writer bufsize {}, queue length {}",
                        String::from_utf8_lossy(w.data()), String::from_utf8_lossy(&text[..]),
                        channel_bufsize, writer_bufsize, queuelen
                    ));
                }
            }
        }
    }
}

#[test]
fn writer_init_fail() {
    let e = io::Error::new(io::ErrorKind::Other, "init err");
    let res = writer_init(5, 2, || Err::<&mut [u8], _>(e), |_| {Ok(())});
    if let Err(e) = res {
        assert_eq!(&format!("{}", e), "init err");
    } else {
        panic!("init should fail");
    }
}

#[test]
fn write_fail() {
    let text = b"The quick brown fox jumps over the lazy dog";
    let len = text.len();

    for channel_bufsize in 1..len {
        for writer_bufsize in 1..len {
            for queuelen in 1..len {
                let w = Writer::new(true, false, writer_bufsize);
                let res = writer(channel_bufsize, queuelen, w, |w| w.write(text));
                if let Err(e) = res {
                    assert_eq!(&format!("{}", e), "write err");
                } else {
                    panic!("write should fail");
                }

                let w = Writer::new(false, true, writer_bufsize);
                let res = writer(channel_bufsize, queuelen, w, |w| w.flush());
                if let Err(e) = res {
                    assert_eq!(&format!("{}", e), "flush err");
                } else {
                    panic!("flush should fail");
                }
            }
        }
    }
}

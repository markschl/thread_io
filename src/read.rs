//! Send data from an io::Read in a background thread to a reader in the main thread.

use std::mem::replace;
use std::io::{self, Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};

use crossbeam;


#[derive(Debug)]
struct Buffer {
    data: Box<[u8]>,
    pos: usize,
    end: usize,
    // read returned n = 0 -> EOF
    eof: bool,
}

impl Buffer {
    fn new(size: usize) -> Buffer {
        assert!(size > 0);
        Buffer {
            data: vec![0; size].into_boxed_slice(),
            pos: 0,
            end: 0,
            eof: false,
        }
    }

    fn read(&mut self, mut buf: &mut [u8]) -> usize {
        let n = buf.write(&self.data[self.pos..self.end]).unwrap();
        self.pos += n;
        n
    }

    /// Fill the whole buffer, unless EOF occurs. ErrorKind::Interrupted is not handled.
    fn refill<R: Read>(&mut self, mut reader: R) -> io::Result<()> {
        let mut n_read = 0;
        let mut buf = &mut *self.data;
        while !buf.is_empty() {
            let n = reader.read(buf)?;
            if n == 0 {
                self.eof = true;
                break
            }
            let tmp = buf;
            buf = &mut tmp[n..];
            n_read += n;
        }
        self.pos = 0;
        self.end = n_read;
        Ok(())
    }
}


#[derive(Debug)]
pub struct Reader {
    full_recv: Receiver<io::Result<Buffer>>,
    empty_send: Sender<Option<Buffer>>,
    buffer: Buffer,
}

impl Reader {
    fn done(&self) {
        self.empty_send.send(None).ok();
    }

    /// return errors that may still be in the queue
    fn get_errors(&self) -> io::Result<()> {
        for res in &self.full_recv {
            res?;
        }
        Ok(())
    }
}

impl io::Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = self.buffer.read(buf);
            if n > 0 {
                return Ok(n);
            } else if self.buffer.eof {
                return Ok(0);
            } else {
                let data = self.full_recv.recv().ok().unwrap()?;
                let old = replace(&mut self.buffer, data);
                self.empty_send.send(Some(old)).ok();
            }
        }
    }
}

/// Wraps `reader` in a background thread and provides a reader in the main thread, which
/// obtains data from the background reader.
///
/// **Note**: Errors will not be returned immediately, but after `queuelen`
/// reads, or after reading is finished and the closure ends. The reader in the
/// background thread will stop if an error occurrs, except for errors of kind
/// `ErrorKind::Interrupted`. In this case, reading continues in the background,
/// but the error is still returned.
///
/// # Example:
///
/// ```
/// # extern crate thread_io;
/// use thread_io::read::reader;
/// use std::io::Read;
///
/// # fn main() {
/// let text = b"The quick brown fox jumps over the lazy dog";
/// let mut target = vec![];
///
/// reader(16, 2, &text[..], |rdr| {
///     rdr.read_to_end(&mut target)
/// }).expect("read failed");
///
/// assert_eq!(target.as_slice(), &text[..]);
/// # }
/// ```
pub fn reader<R, F, O, E>(bufsize: usize, queuelen: usize, reader: R, func: F) -> Result<O, E>
where
    F: FnOnce(&mut Reader) -> Result<O, E>,
    R: io::Read + Send,
    E: Send + From<io::Error>
{
    reader_init(bufsize, queuelen, || Ok(reader), func)
}

/// Like `reader()`, but the wrapped reader is initialized using a closure  (`init_reader`)
/// in the background thread. This allows using readers that don't implement `Send`
///
/// # Example:
///
/// ```
/// #![feature(optin_builtin_traits)]
/// # extern crate thread_io;
///
/// use thread_io::read::reader_init;
/// use std::io::{self, Read};
///
/// struct NotSendableReader<'a>(&'a [u8]);
///
/// impl<'a> !Send for NotSendableReader<'a> {}
///
/// impl<'a> Read for NotSendableReader<'a> {
///     fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
///         self.0.read(buf)
///     }
/// }
///
/// # fn main() {
/// let text = b"The quick brown fox jumps over the lazy dog";
/// let mut read = vec![];
///
/// reader_init(16, 2, || Ok(NotSendableReader(&text[..])), |rdr| {
///     rdr.read_to_end(&mut read)
/// }).expect("read failed");
///
/// assert_eq!(read.as_slice(), &text[..]);
/// # }
/// ```
pub fn reader_init<R, I, F, O, E>(
    bufsize: usize,
    queuelen: usize,
    init_reader: I,
    func: F,
) -> Result<O, E>
where
    I: Send + FnOnce() -> Result<R, E>,
    F: FnOnce(&mut Reader) -> Result<O, E>,
    R: io::Read,
    E: Send + From<io::Error>
{
    assert!(queuelen >= 1);

    let (full_send, full_recv): (Sender<io::Result<Buffer>>, _) = channel();
    let (empty_send, empty_recv): (Sender<Option<Buffer>>, _) = channel();
    for _ in 0..queuelen {
        empty_send.send(Some(Buffer::new(bufsize))).ok();
    }

    crossbeam::scope(|scope| {
        let handle = scope.spawn(move || {
            let mut reader = init_reader()?;
            while let Ok(Some(mut buffer)) = empty_recv.recv() {
                if let Err(e) = buffer.refill(&mut reader) {
                    let do_break = e.kind() != io::ErrorKind::Interrupted;
                    full_send.send(Err(e)).ok();
                    if do_break {
                        break;
                    }
                } else {
                    full_send.send(Ok(buffer)).ok();
                }
            }
            Ok::<_, E>(())
        });

        let mut reader = Reader {
            full_recv: full_recv,
            empty_send: empty_send,
            buffer: Buffer::new(bufsize),
        };

        let out = func(&mut reader)?;

        reader.done();

        handle.join().unwrap()?;

        reader.get_errors()?;

        Ok(out)
    })
}

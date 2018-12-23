//! Send data from an io::Read in a background thread to a reader in the main thread.

use std::io::{self, Read, Cursor};
#[cfg(not(feature = "crossbeam_channel"))]
use std::sync::mpsc::{channel, Receiver, Sender};
#[cfg(feature = "crossbeam_channel")]
use crossbeam::channel::{unbounded as channel, Receiver, Sender};

use crossbeam;


#[derive(Debug)]
struct Buffer {
    data: Box<[u8]>,
    end: usize,
    // io::ErrorKind::Interrupted
    interrupted: bool,
}

impl Buffer {
    #[inline]
    fn new(size: usize) -> Buffer {
        assert!(size > 0);
        Buffer {
            data: vec![0; size].into_boxed_slice(),
            end: 0,
            interrupted: false,
        }
    }

    /// Fill the whole buffer
    // if n_read > 0, then read() will called again, and EOF will only
    // be returned then (assuming that read() will again return n == 0)
    #[inline]
    fn refill<R: Read>(&mut self, mut reader: R) -> io::Result<()> {

        let mut n_read = 0;
        let mut buf = &mut *self.data;
        self.interrupted = false;

        while !buf.is_empty() {
            match reader.read(buf) {
                Ok(n) => {
                    if n == 0 {
                        // EOF
                        break;
                    }
                    let tmp = buf;
                    buf = &mut tmp[n..];
                    n_read += n;
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {
                    self.interrupted = true;
                    break;
                }
                Err(e) => return Err(e),
            };
        }

        self.end = n_read;

        Ok(())
    }
}


#[derive(Debug)]
pub struct Reader {
    full_recv: Receiver<io::Result<Buffer>>,
    empty_send: Sender<Option<Buffer>>,
    buffer: Option<Buffer>,
    pos: usize,
}

impl Reader {
    #[inline]
    fn new(full_recv: Receiver<io::Result<Buffer>>, empty_send: Sender<Option<Buffer>>) -> Self {
        Reader {
            full_recv,
            empty_send,
            buffer: None,
            pos: 0,
        }
    }

    #[inline]
    fn done(&self) {
        self.empty_send.send(None).ok();
    }

    /// return errors that may still be in the queue. This is still possible even if the
    /// sender was dropped, as they are in the queue buffer (see docs for std::mpsc::Receiver::recv).
    #[inline]
    fn get_errors(&self) -> io::Result<()> {
        for res in &self.full_recv {
            res?;
        }
        Ok(())
    }

    // assumes that self.buffer is not None. Returns a tuple of the read result
    // and a flag indicating if a new buffer should be received (cannot be done
    // here due to borrow checker)
    #[inline]
    fn _read(&mut self, buf: &mut [u8]) -> (io::Result<usize>, bool) {

        let source = self.buffer.as_mut().unwrap();

        if source.interrupted && self.pos == source.end {
            return (Err(io::Error::from(io::ErrorKind::Interrupted)), true);
        }

        let n = Cursor::new(&source.data[self.pos..source.end]).read(buf).unwrap();
        self.pos += n;

        (Ok(n), self.pos == source.end && !source.interrupted)
    }
}

impl io::Read for Reader {

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {

        if self.buffer.is_none() {
            self.buffer = Some(self.full_recv.recv().ok().unwrap()?);
        }

        let (rv, recv_next) = self._read(buf);

        if recv_next {
            self.empty_send.send(self.buffer.take()).ok();
            self.pos = 0;
        }

        rv
    }
}

/// Wraps `reader` in a background thread and provides a reader in the main thread, which
/// obtains data from the background reader.
///
/// The background reader fills a buffer of a given size (`bufsize`) and submits
/// the data to the main thread through a channel. The length of its queue can
/// be configured using the `queuelen` parameter (must be >= 1). As a consequence,
/// errors will not be returned immediately, but after `queuelen`
/// reads, or after reading is finished and the closure ends. The reader in the
/// background thread will stop if an error occurrs, except for errors of kind
/// `io::ErrorKind::Interrupted`. In this case, reading continues in the background,
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
        let handle = scope.spawn(move |_| {

            let mut reader = init_reader()?;

            while let Ok(Some(mut buffer)) = empty_recv.recv() {
                match buffer.refill(&mut reader) {
                    Ok(_) => {
                        full_send.send(Ok(buffer)).ok();
                    }
                    Err(e) => {
                        full_send.send(Err(e)).ok();
                        break;
                    }
                }
            }

            Ok::<_, E>(())
        });

        let mut reader = Reader::new(full_recv, empty_send);

        let out = func(&mut reader)?;

        reader.done();

        handle.join().unwrap()?;

        reader.get_errors()?;

        Ok(out)
    }).unwrap()
}

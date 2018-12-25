//! This module contains functions for reading in a background thread.

#[cfg(feature = "crossbeam_channel")]
use crossbeam::channel::{unbounded as channel, Receiver, Sender};
use std::io::{self, Cursor, Read};
#[cfg(not(feature = "crossbeam_channel"))]
use std::sync::mpsc::{channel, Receiver, Sender};

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

    /// Fill the whole buffer, using multiple reads if necessary. This means, that upon EOF,
    /// read may be called again once before n = 0 is returned in the main thread.
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

/// The reader in the main thread
#[derive(Debug)]
pub struct Reader {
    full_recv: Receiver<io::Result<Buffer>>,
    empty_send: Sender<Option<Buffer>>,
    buffer: Option<Buffer>,
    pos: usize,
}

impl Reader {
    #[inline]
    fn new(
        full_recv: Receiver<io::Result<Buffer>>,
        empty_send: Sender<Option<Buffer>>,
        bufsize: usize,
        queuelen: usize,
    ) -> Self {
        for _ in 0..queuelen {
            empty_send.send(Some(Buffer::new(bufsize))).ok();
        }

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

        let n = Cursor::new(&source.data[self.pos..source.end])
            .read(buf)
            .unwrap();
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

#[derive(Debug)]
struct BackgroundReader {
    empty_recv: Receiver<Option<Buffer>>,
    full_send: Sender<io::Result<Buffer>>,
}

impl BackgroundReader {
    #[inline]
    fn new(empty_recv: Receiver<Option<Buffer>>, full_send: Sender<io::Result<Buffer>>) -> Self {
        BackgroundReader {
            empty_recv,
            full_send,
        }
    }

    #[inline]
    fn serve<R: Read>(&mut self, mut reader: R) {
        while let Ok(Some(mut buffer)) = self.empty_recv.recv() {
            match buffer.refill(&mut reader) {
                Ok(_) => {
                    self.full_send.send(Ok(buffer)).ok();
                }
                Err(e) => {
                    self.full_send.send(Err(e)).ok();
                    break;
                }
            }
        }
    }
}

/// Sends `reader` to a background thread and provides a reader in the main thread, which
/// obtains data from the background reader.
///
/// The background reader fills buffers of a given size (`bufsize`) and submits
/// them to the main thread through a channel. The queue length of the channel can
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
/// use thread_io::read::reader;
/// use std::io::Read;
///
/// let text = b"The quick brown fox jumps over the lazy dog";
/// let mut target = vec![];
///
/// reader(16, 2, &text[..], |rdr| {
///     rdr.read_to_end(&mut target)
/// }).expect("read failed");
///
/// assert_eq!(target.as_slice(), &text[..]);
/// ```
pub fn reader<R, F, O, E>(bufsize: usize, queuelen: usize, reader: R, func: F) -> Result<O, E>
where
    F: FnOnce(&mut Reader) -> Result<O, E>,
    R: io::Read + Send,
    E: Send + From<io::Error>,
{
    reader_init(bufsize, queuelen, || Ok(reader), func)
}

/// Like `reader()`, but the wrapped reader is initialized using a closure  (`init_reader`)
/// in the background thread. This allows using readers that don't implement `Send`
///
/// # Example:
///
/// ```
/// use thread_io::read::reader_init;
/// use std::io::{self, Read};
///
/// let mut input = io::stdin();
///
/// // StdinLock does not implement Send
/// reader_init(16, 2, || Ok(input.lock()), |rdr| {
///     let mut s = String::new();
///     let _ = rdr.read_to_string(&mut s).expect("read error");
///     // ...
///     Ok::<_, io::Error>(())
/// }).expect("read failed");
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
    E: Send + From<io::Error>,
{
    assert!(queuelen >= 1);
    assert!(bufsize > 0);

    let (full_send, full_recv) = channel();
    let (empty_send, empty_recv) = channel();

    let mut reader = Reader::new(full_recv, empty_send, bufsize, queuelen);
    let mut background_reader = BackgroundReader::new(empty_recv, full_send);

    crossbeam::scope(|scope| {
        let handle = scope.spawn(move |_| {
            let mut inner = init_reader()?;
            background_reader.serve(&mut inner);
            Ok::<_, E>(())
        });

        let out = func(&mut reader)?;

        reader.done();

        handle.join().unwrap()?;

        reader.get_errors()?;

        Ok(out)
    })
    .unwrap()
}

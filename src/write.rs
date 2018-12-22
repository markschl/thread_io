//! Send data from an io::Write in the main thread to a writer in a background thread.

use std::mem::replace;
use std::io::{self, Write};
use std::sync::mpsc::{channel, Receiver, Sender};

use crossbeam;


#[derive(Debug)]
enum Message {
    Buffer(io::Cursor<Box<[u8]>>),
    Flush,
    Done
}

#[derive(Debug)]
pub struct Writer {
    empty_recv: Receiver<io::Result<Box<[u8]>>>,
    full_send: Sender<Message>,
    buffer: io::Cursor<Box<[u8]>>,
}

impl Writer {
    fn send_to_thread(&mut self) -> io::Result<()> {
        if let Ok(empty) = self.empty_recv.recv() {
            let full = replace(&mut self.buffer, io::Cursor::new(empty?));
            if self.full_send.send(Message::Buffer(full)).is_err() {
                self.get_errors()?;
            }
        } else {
            self.get_errors()?;
        }
        Ok(())
    }

    fn done(&mut self) -> io::Result<()> {
        // send last buffer
        self.send_to_thread()?;
        self.full_send.send(Message::Done).ok();
        Ok(())
    }

    // return errors that may still be in the queue
    fn get_errors(&self) -> io::Result<()> {
        for res in &self.empty_recv {
            res?;
        }
        Ok(())
    }
}

impl Write for Writer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut written = 0;
        while written < buffer.len() {
            let n = self.buffer.write(&buffer[written..])?;
            written += n;
            if n == 0 {
                self.send_to_thread()?;
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.full_send.send(Message::Flush).ok();
        Ok(())
    }
}


/// Sends `writer` to a new thread and provides another writer in the main thread, which sends
/// its data to the background.
///
/// **Note**: Errors will not be returned immediately, but after `queuelen`
/// writes, or after writing is finished and the closure ends.
/// Also note that the last `write()` might be done **after** the closure
/// has ended, so calling `flush` within the closure is too early.
/// In that case, flushing (or other finalizing actions) can be done in the `finish` closure
/// supplied to `writer_with_finish()`.
///
/// # Example:
///
/// ```
/// # extern crate thread_io;
/// use thread_io::write::writer;
/// use std::io::Write;
///
/// # fn main() {
/// let text = b"The quick brown fox jumps over the lazy dog";
/// let mut buf = vec![0; text.len()];
///
/// writer(16, 2, &mut buf[..], |writer| {
///     writer.write_all(&text[..])
/// }).expect("write failed");
///
/// assert_eq!(&buf[..], &text[..]);
/// # }
/// ```
pub fn writer<W, F, O, E>(bufsize: usize, queuelen: usize, writer: W, func: F) -> Result<O, E>
where
    F: FnOnce(&mut Writer) -> Result<O, E>,
    W: Write + Send,
    E: Send + From<io::Error>
{
    writer_init(bufsize, queuelen, || Ok(writer), func)
}

/// Like `writer()`, but the wrapped writer is initialized using a closure  (`init_writer`)
/// in the background thread. This allows using writers that don't implement `Send`
///
/// # Example:
///
/// ```
/// #![feature(optin_builtin_traits)]
/// # extern crate thread_io;
/// use thread_io::write::writer_init;
/// use std::io::{self, Write};
///
/// # fn main() {
/// let text = b"The quick brown fox jumps over the lazy dog";
/// let mut buf = vec![0; text.len()];
///
/// struct NotSendableWriter<'a>(&'a mut [u8]);
///
/// impl<'a> !Send for NotSendableWriter<'a> {}
///
/// impl<'a> Write for NotSendableWriter<'a> {
///     fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
///         self.0.write(buf)
///     }
///
///     fn flush(&mut self) -> io::Result<()> {
///         Ok(())
///     }
/// }
///
/// writer_init(16, 2, || Ok(NotSendableWriter(&mut buf[..])), |writer| {
///     writer.write_all(&text[..])
/// }).expect("write failed");
///
/// assert_eq!(&buf[..], &text[..]);
/// # }
/// ```
pub fn writer_init<W, I, F, O, E>(
    bufsize: usize,
    queuelen: usize,
    init_writer: I,
    func: F,
) -> Result<O, E>
where
    I: Send + FnOnce() -> Result<W, E>,
    F: FnOnce(&mut Writer) -> Result<O, E>,
    W: Write,
    E: Send + From<io::Error>
{
    writer_init_finish(bufsize, queuelen, init_writer, func, |_| ()).map(|(o, _)| o)
}

/// Like `writer_init()`, but with another closure that takes the writer by value
/// before it goes out of scope (and there is no error). Useful e.g. with encoders
/// for compressed data that require calling a `finish` function. If the writer
/// implements `Send`, it is also possible to return the wrapped writer back to
/// the main thread.
///
/// # Example:
///
/// ```
/// # extern crate thread_io;
/// use thread_io::write::writer_init_finish;
/// use std::io::Write;
///
/// # fn main() {
/// let text = b"The quick brown fox jumps over the lazy dog";
/// let output = vec![];
///
/// // `output` is moved
/// let (_, output) = writer_init_finish(16, 2,
///     || Ok(output),
///     |out| out.write_all(&text[..]),
///     |out| out // output is returned to main thread
/// ).expect("write failed");
///
/// println!("a: {}", std::str::from_utf8(&output[..]).unwrap());
/// assert_eq!(&output[..], &text[..]);
/// # }
/// ```
pub fn writer_init_finish<W, I, F, O, F2, O2, E>(
    bufsize: usize,
    queuelen: usize,
    init_writer: I,
    func: F,
    finish: F2
) -> Result<(O, O2), E>
where
    I: Send + FnOnce() -> Result<W, E>,
    F: FnOnce(&mut Writer) -> Result<O, E>,
    W: Write,
    F2: Send + FnOnce(W) -> O2,
    O2: Send,
    E: Send + From<io::Error>
{
    assert!(queuelen >= 1);
    assert!(bufsize > 0);

    let (full_send, full_recv): (Sender<Message>, _) =
        channel();
    let (empty_send, empty_recv) = channel();
    for _ in 0..queuelen {
        empty_send
            .send(Ok(vec![0; bufsize].into_boxed_slice()))
            .ok();
    }

    crossbeam::scope(|scope| {
        let handle = scope.spawn::<_, Result<_, E>>(move |_| {
            let mut writer = init_writer()?;

            while let Ok(msg) = full_recv.recv() {
                match msg {
                    Message::Buffer(buf) => {
                        let pos = buf.position() as usize;
                        let buffer = buf.into_inner();
                        let res = writer.write_all(&buffer[..pos]);
                        let is_err = res.is_err();
                        empty_send.send(res.map(|_| buffer)).ok();
                        if is_err {
                            return Ok(None);
                        }
                    }
                    Message::Flush => {
                        if let Err(e) = writer.flush() {
                            empty_send.send(Err(e)).ok();
                            return Ok(None);
                        }
                    }
                    Message::Done => break
                }
            }
            // writing finished witout error
            Ok(Some(finish(writer)))
        });

        let mut writer = Writer {
            empty_recv: empty_recv,
            full_send: full_send,
            buffer: io::Cursor::new(vec![0; bufsize].into_boxed_slice()),
        };

        let out = func(&mut writer)?;

        writer.done()?;

        let of = handle.join().unwrap()?;

        writer.get_errors()?;

        Ok((out, of.unwrap()))
    }).unwrap()
}

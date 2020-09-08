//! This module contains functions for writing in a background thread.

use std::io::{self, Write};
use std::mem::replace;

#[cfg(feature = "crossbeam_channel")]
use crossbeam::channel::{unbounded as channel, Receiver, Sender};
#[cfg(not(feature = "crossbeam_channel"))]
use std::sync::mpsc::{channel, Receiver, Sender};

use crossbeam;

#[derive(Debug)]
enum Message {
    Buffer(io::Cursor<Box<[u8]>>),
    Flush,
    Done,
}

/// The writer in the main thread
#[derive(Debug)]
pub struct Writer {
    empty_recv: Receiver<io::Result<Box<[u8]>>>,
    full_send: Sender<Message>,
    buffer: io::Cursor<Box<[u8]>>,
}

impl Writer {
    #[inline]
    fn new(
        empty_recv: Receiver<io::Result<Box<[u8]>>>,
        full_send: Sender<Message>,
        bufsize: usize,
    ) -> Self {
        let buffer = io::Cursor::new(vec![0; bufsize].into_boxed_slice());

        Writer {
            empty_recv,
            full_send,
            buffer,
        }
    }

    #[inline]
    fn send_to_background(&mut self) -> io::Result<()> {
        if let Ok(empty) = self.empty_recv.recv() {
            let full = replace(&mut self.buffer, io::Cursor::new(empty?));
            if self.full_send.send(Message::Buffer(full)).is_err() {
                self.get_errors()?;
            }
            Ok(())
        } else {
            self.get_errors()?;
            // If we reach this point, we couldn't communicate with the background writer
            // but there were no errors recorded in the queue. BrokenPipe seems to closest error to return.
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }

    #[inline]
    fn done(&mut self) -> io::Result<()> {
        // send last buffer
        self.send_to_background()?;
        self.full_send.send(Message::Done).ok();
        Ok(())
    }

    // return errors that may still be in the queue
    #[inline]
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
                self.send_to_background()?;
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.full_send.send(Message::Flush).ok();
        Ok(())
    }
}

#[derive(Debug)]
struct BackgroundWriter {
    full_recv: Receiver<Message>,
    empty_send: Sender<io::Result<Box<[u8]>>>,
}

impl BackgroundWriter {
    #[inline]
    fn new(
        full_recv: Receiver<Message>,
        empty_send: Sender<io::Result<Box<[u8]>>>,
        bufsize: usize,
        queuelen: usize,
    ) -> Self {
        for _ in 0..queuelen {
            empty_send
                .send(Ok(vec![0; bufsize].into_boxed_slice()))
                .ok();
        }
        BackgroundWriter {
            full_recv,
            empty_send,
        }
    }

    #[inline]
    fn listen<W: Write>(&mut self, mut writer: W) -> bool {
        while let Ok(msg) = self.full_recv.recv() {
            match msg {
                Message::Buffer(buf) => {
                    let pos = buf.position() as usize;
                    let buffer = buf.into_inner();
                    let res = writer.write_all(&buffer[..pos]);
                    let is_err = res.is_err();
                    self.empty_send.send(res.map(|_| buffer)).ok();
                    if is_err {
                        return false;
                    }
                }
                Message::Flush => {
                    if let Err(e) = writer.flush() {
                        self.empty_send.send(Err(e)).ok();
                        return false;
                    }
                }
                Message::Done => break,
            }
        }
        true
    }
}

/// Sends `writer` to a background thread and provides another writer in the main thread, which
/// then submits its data to the background writer.
///
/// The writer in the closure (`func`) fills buffers of a given size (`bufsize`) and submits
/// them to the background writer through a channel. The queue length of the channel can
/// be configured using the `queuelen` parameter (must be >= 1). As a consequence, errors will
/// not be returned immediately, but after `queuelen` writes, or after writing is finished and
/// the closure ends.
///
/// Also note that the last `write()` might be done **after** the closure has ended, calling
/// `flush` within the closure is therefore too early. Flushing or other finalizing actions
/// can be done in the `finish` closure supplied to `writer_finish()` or `writer_init_finish()`.
///
/// # Example:
///
/// ```
/// use thread_io::write::writer;
/// use std::io::Write;
///
/// let text = b"The quick brown fox jumps over the lazy dog";
/// let mut buf = vec![0; text.len()];
///
/// writer(16, 2, &mut buf[..], |writer| {
///     writer.write_all(&text[..])
/// }).expect("write failed");
///
/// assert_eq!(&buf[..], &text[..]);
/// ```
pub fn writer<W, F, O, E>(bufsize: usize, queuelen: usize, writer: W, func: F) -> Result<O, E>
where
    F: FnOnce(&mut Writer) -> Result<O, E>,
    W: Write + Send,
    E: Send + From<io::Error>,
{
    writer_init(bufsize, queuelen, || Ok(writer), func)
}

/// Like `writer()`, but the wrapped writer is initialized using a closure  (`init_writer()`)
/// in the background thread. This allows using writers that don't implement `Send`
///
/// # Example:
///
/// ```
/// use thread_io::write::writer_init;
/// use std::io::{self, Write};
///
/// let text = b"The quick brown fox jumps over the lazy dog";
/// let mut output = io::stdout();
///
/// // StdoutLock does not implement Send
/// writer_init(16, 2, || Ok(output.lock()), |writer| {
///     writer.write_all(&text[..])
/// }).expect("write failed");
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
    E: Send + From<io::Error>,
{
    writer_init_finish(bufsize, queuelen, init_writer, func, |_| ()).map(|(o, _)| o)
}

/// Like `writer`, but accepts another closure taking the wrapped writer by value before it
/// goes out of scope (if there is no error). Useful for performing finalizing actions such
/// as flusing, or calling a `finish()` function, required by many encoders for compressed data.
///
/// If the writer implements `Send`, it is also possible to return the wrapped writer back to the
/// main thread.
///
/// The output values of `func` and the `finish` closure are returned in a tuple.
///
/// # Example:
///
/// ```
/// use thread_io::write::writer_finish;
/// use std::io::Write;
///
/// let text = b"The quick brown fox jumps over the lazy dog";
/// let mut output = vec![];
///
/// // `output` is moved to background thread
/// let (_, output) = writer_finish(16, 2, output,
///     |out| out.write_all(&text[..]),
///     |out| out // output is returned to main thread
/// ).expect("write failed");
///
/// assert_eq!(&output[..], &text[..]);
/// ```
pub fn writer_finish<W, F, O, F2, O2, E>(
    bufsize: usize,
    queuelen: usize,
    writer: W,
    func: F,
    finish: F2,
) -> Result<(O, O2), E>
where
    F: FnOnce(&mut Writer) -> Result<O, E>,
    W: Write + Send,
    F2: Send + FnOnce(W) -> O2,
    O2: Send,
    E: Send + From<io::Error>,
{
    writer_init_finish(bufsize, queuelen, || Ok(writer), func, finish)
}

/// This method takes both an initializing closure (see `writer_init`) and one for finalizing
/// and returning data back to the main thread (see `writer_finish`).alloc
///
/// The output values of `func` and the `finish` closure are returned as a tuple.
pub fn writer_init_finish<W, I, F, O, F2, O2, E>(
    bufsize: usize,
    queuelen: usize,
    init_writer: I,
    func: F,
    finish: F2,
) -> Result<(O, O2), E>
where
    I: Send + FnOnce() -> Result<W, E>,
    F: FnOnce(&mut Writer) -> Result<O, E>,
    W: Write,
    F2: Send + FnOnce(W) -> O2,
    O2: Send,
    E: Send + From<io::Error>,
{
    assert!(queuelen >= 1);
    assert!(bufsize > 0);

    let (full_send, full_recv) = channel();
    let (empty_send, empty_recv) = channel();

    let mut writer = Writer::new(empty_recv, full_send, bufsize);
    let mut background_writer = BackgroundWriter::new(full_recv, empty_send, bufsize, queuelen);

    crossbeam::scope(|scope| {
        let handle = scope.spawn::<_, Result<_, E>>(move |_| {
            let mut inner = init_writer()?;
            if background_writer.listen(&mut inner) {
                // writing finished witout error
                return Ok(Some(finish(inner)));
            }
            Ok(None)
        });

        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| func(&mut writer)));

        let writer_result = writer.done();

        let handle = handle.join();

        // Prefer errors from the background thread. This doesn't include actual I/O errors from the writing
        // because those are sent via the channel to the main thread. Instead, it returns errors from init_writer
        // or panics from the writing thread. If either of those happen, writing in the main thread will fail
        // but we want to return the underlying reason.
        let of = crate::unwrap_or_resume_unwind(handle)?;
        let out = crate::unwrap_or_resume_unwind(out)?;

        // Report write errors that happened after the main thread stopped writing.
        writer_result?;
        writer.get_errors()?;

        Ok((out, of.unwrap()))
    })
    .unwrap()
}

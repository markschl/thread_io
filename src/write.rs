//! This module contains functions for writing in a background thread.
//!
//! * The simplest to use is the [`writer`](fn.writer.html) function. It accepts
//! any `io::Write` instance that implements `Send`.
//! * The [`writer_init`](fn.writer_init.html) function handles cases where the
//! wrapped writer cannot be sent safely across the thread boundary by providing
//! a closure for initializing the writer in the background thread.
//! * [`writer_finish`](fn.writer_finish.html) and
//! [`writer_init_finish`](fn.writer_init_finish.html) provide an additional
//! `finish` closure, which allows for executing final code after *all* writing
//! finished or also for returning the wrapped writer to the main thread.
//!
//! # Error handling
//!
//! * `io::Error`s occurring during writing in the background are returned by
//!   the `write` method of the [writer in the main thread](struct.Writer.html)
//!   as expected, but with a delay of at east one call.
//! * The `func` closure running in the main thread allows returning errors of
//!   any type. However, in contrast to the functions in the `read` module, the
//!   error needs to implement `From<io::Error>` because additional `write`
//!   calls can happen in the background **after** `func` is already finished,
//!   and eventual errors have to be returned as well.
//! * If an error is returned from `func` in the main thread, and around the
//!   same time a writing error happens in the background thread, which does not
//!   reach the main thread due to the reporting delay, the latter will be
//!   discarded and the error from `func` returned instead.
//!   Due to this, there is no guarantee on how much data is still written
//!   after the custom error occurs.
//! * *panics* in the background writer are correctly forwarded to the main
//!   thread, but are also given lower priority if an error is returned from
//!   `func`.
//!
//! # Flushing
//!
//! Trying to mimick the expected functionality of `io::Write` as closely as
//! possible, the [writer](struct.Writer.html) provided in the `func`
//! closure in the main thread also forwards `flush()` calls to the background
//! writer. `write` and `flush` calls are guaranteed to be in the same order as
//! in the main thread. A `flush` call will always trigger the writer in the
//! main thread to send any buffered data to the background *before* the actual
//! flushing is queued.
//!
//! In addition, `flush()` is **always** called after all writing is done.
//! It is assumed that usually no more data will be written after this, and a
//! call to `flush()` ensures that that possible flushing errors don't go
//! unnoticed like they would if the file is closed automatically when dropped.

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

    /// Sends the current buffer to the background thread for writing. Returns
    /// `false` if the background thread has terminated, which could have the
    /// following reasons:
    /// 1) There was a write/flush error or panic. These have to be handled
    ///    accordingly, here we do nothing more.
    /// 2) Message::Done has already been sent, send_to_background() was called
    ///    after done()). This should not happen in the current code.
    /// 3) Writer initialization failed.
    #[inline]
    fn send_to_background(&mut self) -> io::Result<bool> {
        if let Ok(empty) = self.empty_recv.recv() {
            let full = replace(&mut self.buffer, io::Cursor::new(empty?));
            if self.full_send.send(Message::Buffer(full)).is_ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[inline]
    fn done(&mut self) -> io::Result<()> {
        // send last buffer
        self.send_to_background()?;
        // Tell the background writer to finish up if it didn't already
        self.full_send.send(Message::Done).ok();
        Ok(())
    }

    // Checks all items in the empty_recv queue for a possible error,
    // throwing away all buffers. This should thus only be called at the
    // very end.
    #[inline]
    fn fetch_error(&self) -> io::Result<()> {
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
                if !self.send_to_background()? {
                    break;
                }
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send_to_background()?;
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

/// Sends `writer` to a background thread and provides another writer in the
/// main thread, which then submits its data to the background writer.
///
/// The writer in the closure (`func`) fills buffers of a given size (`bufsize`)
/// and submits them to the background writer through a channel. The queue
/// length of the channel can be configured using the `queuelen` parameter (must
/// be ≥ 1). As a consequence, errors will not be returned immediately, but
/// after some delay, or after writing is finished and the closure ends.
///
/// Also note that the last `write()` in the background can happen **after** the
/// closure has ended. Finalizing actions should be done in the `finish` closure
/// supplied to [`writer_finish`](fn.writer_finish.html) or
/// [`writer_init_finish`](fn.writer_init_finish.html).
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

/// Like [`writer`](fn.writer.html), but the wrapped writer is initialized in a
/// closure (`init_writer()`) in the background thread. This allows using
/// writers that don't implement `Send`.
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

/// Like `writer`, but accepts another closure taking the wrapped writer by
/// value before it goes out of scope (and there is no error).
/// Useful for performing finalizing actions or returning the wrapped writer to
/// the main thread (if it implements `Send`).
///
/// The output values of `func` and the `finish` closure are returned in a
/// tuple.
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

/// This method takes both an initializing closure (see
/// [`writer_init`](fn.writer_init.html)), and a closure for finalizing or
/// returning data back to the main thread (see
/// [`writer_finish`](fn.writer_finish.html)).
///
/// The output values of `func` and the `finish` closure are returned as a
/// tuple.
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

        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let out = func(&mut writer)?;
            // we always flush before returning
            writer.flush()?;
            Ok::<_, E>(out)
        }));

        let writer_result = writer.done();

        let handle = handle.join();

        // Prefer errors from the background thread. This doesn't include actual
        // I/O errors from the writing because those are sent via the channel to
        // the main thread. Instead, it returns errors from init_writer or
        // panics from the writing thread. If either of those happen, writing in
        // the main thread will fail but we want to return the underlying reason.
        let of = crate::unwrap_or_resume_unwind(handle)?;
        let out = crate::unwrap_or_resume_unwind(out)?;

        // Report write errors that happened after the main thread stopped writing.
        writer_result?;
        // Return errors that may have occurred when writing the last few chunks.
        writer.fetch_error()?;

        Ok((out, of.unwrap()))
    })
    .unwrap()
}

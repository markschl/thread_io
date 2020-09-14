# thread_io

[![docs.rs](https://docs.rs/thread_io/badge.svg)](https://docs.rs/thread_io/latest/thread_io)
[![crates.io](https://img.shields.io/crates/v/thread_io.svg)](https://crates.io/crates/thread_io)
[![build status](https://api.travis-ci.org/markschl/thread_io.svg?branch=master)](https://travis-ci.org/markschl/thread_io)

This crate allows to easily wrap readers and writers in a background thread.
This can be useful e.g. with readers and writers for compression formats to 
reduce load on the main thread.

`thread_io` uses channels (optionally from the
[crossbeam crate](https://docs.rs/crossbeam/latest/crossbeam/channel/index.html))
for communicating and exchanging chunks of data with the background reader /
writer.

**[Reader API documentation](https://docs.rs/thread_io/latest/thread_io/read)**

**[Writer API documentation](https://docs.rs/thread_io/latest/thread_io/write)**

## Examples

### Reading

The following code counts the number of lines containing *spam* in a gzip 
compressed file. Decompression is done in a background thread using the `flate2`
library, and the decompressed data is sent to a reader supplied to a closure in
the main thread. The speed gain should be highest if decompression and text
searching use about the same amount of CPU time.

The resulting line number should be the same as the output of 
`zcat file.txt.gz | grep 'spam' | wc -l`.

```rust
use io::prelude::*;
use io;
use fs::File;
use thread_io::read::reader;
use flate2::read::GzDecoder;

// size of buffers sent across threads
const BUF_SIZE: usize = 256 * 1024;
// length of queue with buffers pre-filled in background thread
const QUEUE_LEN: usize = 5;

let f = File::open("file.txt.gz").unwrap();
let gz = GzDecoder::new(f);
let search_term = "spam";

let found = reader(
    BUF_SIZE, 
    QUEUE_LEN,  
    gz, 
    |reader| {
        let mut buf_reader = io::BufReader::new(reader);
        let mut found = 0;
        let mut line = String::new();
        while buf_reader.read_line(&mut line)? > 0 {
            if line.contains(search_term) {
                found += 1;
            }
            line.clear();
        }
        Ok::<_, io::Error>(found)
    }
)
.expect("decoding error");

println!("Found '{}' in {} lines.", search_term, found);
```

Note that this is an example for illustration. To increase performance, one
could read lines into a `Vec<u8>` buffer (instead of `String`) and search for
*spam* e.g. using `memchr` from the [memchr crate](https://crates.io/crates/memchr).

The compiler sometimes needs a hint about the exact error type returned from
`func`, in this case this was done by specifying `Ok::<_, io::Error>()` as
return value.

`thread_io::read::reader` requires the underlying reader to implement `Send`.
Unfortunately, this is not always the case, such as with `io::StdinLock`. 
There is the [`thread_io::read::reader_init`](https://docs.rs/thread_io/latest/thread_io/read/fn.reader_init.html)
function to handle such cases.

### Writing

Writing to a gzip compressed file in a background thread works similarly as
reading. The following code writes all lines containing *spam* to a compressed
file. The contents of the compressed output file `file.txt.gz` should be the
same as if running `grep 'spam' file.txt | gzip -c > file.txt.gz`

```rust
use fs::File;
use io::prelude::*;
use io;
use thread_io::write::writer;
use flate2::write::{GzEncoder};
use flate2::Compression;

const BUF_SIZE: usize = 256 * 1024;
const QUEUE_LEN: usize = 5;

let infile = File::open("file.txt").unwrap();
let outfile = File::create("file.txt.gz").unwrap();
let mut gz_out = GzEncoder::new(outfile, Compression::default());
let search_term = "spam";

writer(
    BUF_SIZE,
    QUEUE_LEN,
    &mut gz_out,
    |writer| {
        // This function runs in the main thread, all writes are written to
        // 'gz_out' in the background
        let mut buf_infile = io::BufReader::new(infile);
        let mut line = String::new();
        while buf_infile.read_line(&mut line)? > 0 {
            if line.contains(search_term) {
                writer.write(line.as_bytes()).expect("write error");
                line.clear();
            }
        }
        Ok::<_, io::Error>(())
    },
)
.expect("encoding error");
gz_out.finish().expect("finishing failed");
```

More details on the exact behavior and more flexible functions e.g. for dealing
with *non-Send* writer types can be found in the documentation of the
[write module](https://docs.rs/thread_io/latest/thread_io/write).

After `func` returns, the background writer *always* calls 
`io::Write::flush`, making sure that possible errors when writing remaining
buffered data to the target. With `File`, it is still possible that errors
occurring when syncing OS level buffers to file
[are not caught](https://github.com/rust-lang/rust/issues/32255). This can be
ensured by calling `File::sync_data` *after* the call to 
`thread_io::write::writer` or in the `finish` closure of 
`thread_io::write::writer_finish` or `thread_io::write::writer_init_finish`.

## Notes on errors

Two types of errors may occur when using `thread_io::read::reader` and 
`thread_io::write::writer`:

* **`io::Error`** if the underlying reader fails in a `io::Read::read`
  or `io::Write::write` call. This error cannot be returned *instantly*,
  instead it is pushed to a queue and will be returned in a subsequent read or
  write call. The delay depends on the `queuelen` parameter of the reading / 
  writing and also on the `bufsize` parameter and the size of the reading /
  writing buffer.
* The `func` closure allows returning **custom errors** of any type, which may 
  occur in the user program *after* reading from the background reader or 
  *before* writing to the background writer. With the `thread_io` writer, there 
  is the additional required trait bound `From<io::Error>` due to the way the
  writer works.

Both with reading and writing, custom user errors are prioritized over eventual
`io::Error`s.
For example, it is possible that while parsing a file, a syntax error occurs,
which the programmer returns from the `func` closure. Around the same time, also
`io::Error` occurs, e.g. because the GZIP file is truncated. If this error
is still in the queue, waiting to be reported when the syntax error occurs, the
syntax error will be returned.

After the func closure ends (with or without an error), a signal is placed in
a queue telling the background thread to stop processing. However, `queuelen` 
reads or writes will be done before processing ultimately stops.

## Crossbeam channels

It is possible to use 
[the channel implementation from the crossbeam crate](https://docs.rs/crossbeam/latest/crossbeam/channel/index.html)
by specifying the `crossbeam_channel` feature. However, on my system, I haven't
found any performance gain over using the channels from the standard library.

## Similar projects

[**fastq-rs**](https://github.com/aseyboldt/fastq-rs) provides a very similar
functionality as `thread_io::read::reader` in its
[`thread_reader`](https://docs.rs/fastq/latest/fastq/fn.thread_reader.html)
module.

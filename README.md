# thread_io

[![docs.rs](https://docs.rs/thread_io/badge.svg)](https://docs.rs/thread_io/latest/thread_io)
[![crates.io](https://img.shields.io/crates/v/thread_io.svg)](https://crates.io/crates/thread_io)
[![build status](https://api.travis-ci.org/markschl/thread_io.svg?branch=master)](https://travis-ci.org/markschl/thread_io)

This crate allows to easily wrap readers and writers in a background thread.
This can be useful e.g. with readers and writers for compression formats to reduce load on the
main thread.

**[Documentation](https://docs.rs/thread_io/latest/thread_io)**

## Examples

### Reading

The following code counts the number of lines containing a search term in
a gzip compressed file. Decompression is done in a background thread using
the `flate2` library, and the decompressed data is sent to a reader supplied to
a closure in the main thread. The speed gain should be highest if decompression
and text searching use about the same amount of CPU time.

```rust
use std::io::prelude::*;
use std::io;
use std::fs::File;
use thread_io::read::reader;
use flate2::read::GzDecoder;

// size of buffers sent across threads
let BUF_SIZE = 256 * 1024;
// length of queue with buffers pre-filled in background thread
let QUEUE_LEN = 5;

let f = File::open("file.txt.gz").unwrap();
let gz = GzDecoder::new(f);
let search_term = "spam";

let found = reader(BUF_SIZE, QUEUE_LEN,  gz, |reader| {
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
})
.expect("decoding error");

println!("Found '{}' in {} lines.", search_term, found);
```

Note that the closure in `reader()` allows returning any error type implementing
`From<io::Error>`, not just `io::Error` itself.
On the downside, the compiler sometimes needs a hint about the error type,
as provided with `Ok::<_, io::Error>()`.

The `thread_io::read` module also provides a function for working with
`non-Send` reader types,
[see documentation here](https://docs.rs/thread_io/latest/thread_io/read/).

### Writing

Writing to a gzip compressed file in a background thread works similarly. The
following code writes all lines containing a specific term to a compressed file:

```rust
use std::fs::File;
use std::io::prelude::*;
use std::io;
use thread_io::write::writer_finish;
use flate2::write::{GzEncoder};
use flate2::Compression;

const BUF_SIZE = 256 * 1024;
const QUEUE_LEN = 5;

let infile = File::open("file.txt").unwrap();
let outfile = File::create("file.txt.gz").unwrap();
let gz_out = GzEncoder::new(outfile, Compression::default());
let search_term = "spam";

writer_finish(BUF_SIZE, QUEUE_LEN, gz_out,
    // runs in main thread
    |writer| {
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
    // runs when main closure finished
    |writer| writer.finish()
)
.expect("encoding error");
```

In this example, `writer.finish()` is called in a separate 'finalizing' closure,
which supplies the `GzEncoder` object by value after all writing is done. Besides,
there is also a standard
[writer](https://docs.rs/thread_io/latest/thread_io/read/writer.html) method,
which takes only one closure. However, note that even calling `flush()` on
a writer should be done in a finalizing closure because the last write is
often performed **after** the main closure ends.

[Module documentation](https://docs.rs/thread_io/latest/thread_io/write/)

### Crossbeam channels

It is possible to use [the channel implementation from the crossbeam crate](https://docs.rs/crossbeam/latest/crossbeam/channel/index.html)
by specifying the `crossbeam_channel` feature. However, on my system, I haven't
found any performance gain over using the channels from the standard library.

### Similar projects

[**fastq-rs**](https://github.com/aseyboldt/fastq-rs) provides a very similar
functionality as `thread_io::read::reader` in its
[`thread_reader`](https://docs.rs/fastq/latest/fastq/fn.thread_reader.html)
module.

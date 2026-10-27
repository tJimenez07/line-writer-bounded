# line-writer-bounded

Appends lines to a file and automatically rotates when the file exceeds a configured maximum size.

```rust
use line_writer_bounded::LineWriterBounded;
use std::path::Path;

let mut writer = LineWriterBounded::new("/tmp/app.log", 10_000)?;
writer.write_line("something happened")?;
writer.flush()?;
```

`LineWriterBounded::new(path, max_size)` opens (or creates) the file in append mode and
tracks its size in memory. Each `write_line` appends the given text plus a trailing newline,
then compares the resulting file size against `max_size`. When the size is greater than or
equal to the threshold, the file is flushed, renamed to `path` + `suffix` (default `.1`),
and a fresh file is opened. `with_suffix` changes the rotation suffix.

## Why

Long-running processes that log to a single file eventually fill the disk. This library
provides one small, dependency-free primitive: write a line, rotate when the file gets too
big. It deliberately does not keep generations of rotated files — each rotation overwrites
the previous rotated file. That is the trade-off: you keep exactly two files on disk (the
active log and the most recent rotated copy) and never need a cleanup policy.

## Edge to know about

Rotation is checked **after** the write completes. A single line larger than `max_size`
will be written in full and then the file rotates, leaving that oversized line in the
rotated file. If you need to cap individual line lengths, truncate before calling
`write_line`.

If the file already exists when `LineWriterBounded::new` is called, its current byte size
is read from metadata and used as the starting counter. An existing file at or above
`max_size` will rotate on the next `write_line`.

Run tests with `cargo test --verbose`.

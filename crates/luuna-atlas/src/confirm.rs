//! The y/n confirmation prompt, called only by delete-domain and compact of s5-atlas-lib-3.
//! Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's `writeConfirm` (`delete.go:9-14`).

use std::io::{self, Read};

/// The size of a new `bufio.Reader`'s buffer.
const BUFFER_SIZE: usize = 4096;

/// `writeConfirm(prompt)`: writes `<prompt> (y/N): ` to `stdout`, then does what
/// `bufio.NewReader(in).ReadString('\n')` does with `input`: reads through a fresh
/// 4096-byte buffer until a newline arrives or the input ends (a line longer than the
/// buffer is read on), ignores the error and uses whatever line it has. It strips one
/// trailing `\n` then one `\r`, lowercases with Go's `strings.ToLower` and answers true
/// only for `y` or `yes`; so `y` then end of input is true and an input with no data is
/// false. What the buffer took from `input` beyond the line is lost, so a second call
/// does not see it (a suspected Go defect, kept). The prompt goes to stdout even in JSON
/// mode, as in Go.
pub fn confirm(prompt: &str, input: &mut dyn Read, stdout: &mut String) -> bool {
    stdout.push_str(prompt);
    stdout.push_str(" (y/N): ");
    let line = read_line(input);
    let line = line.strip_suffix(b"\n").unwrap_or(&line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    // ToLower maps no non-ASCII character to an ASCII one, so ASCII case folding decides
    // the same answers.
    line.eq_ignore_ascii_case(b"y") || line.eq_ignore_ascii_case(b"yes")
}

/// `ReadString('\n')` on a new `bufio.Reader`: each read asks for the room left in the
/// buffer, and the read that holds the newline is the last.
fn read_line(input: &mut dyn Read) -> Vec<u8> {
    let mut line = Vec::new();
    let mut buf = [0u8; BUFFER_SIZE];
    let mut filled = 0;
    loop {
        // In Rust a read of zero bytes into a non-empty buffer is the end of the input
        // (Go's io.EOF); an interrupted read is tried again, as Go's file reads are.
        let n = loop {
            match input.read(&mut buf[filled..]) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Ok(n) => break n,
                Err(_) => break 0,
            }
        };
        if n == 0 {
            line.extend_from_slice(&buf[..filled]);
            break;
        }
        let new = &buf[filled..filled + n];
        if let Some(i) = new.iter().position(|&b| b == b'\n') {
            line.extend_from_slice(&buf[..filled + i + 1]);
            break;
        }
        filled += n;
        if filled == BUFFER_SIZE {
            line.extend_from_slice(&buf);
            filled = 0;
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn answer(input: &[u8]) -> bool {
        let mut out = String::new();
        let result = confirm("Delete?", &mut io::Cursor::new(input.to_vec()), &mut out);
        assert_eq!(out, "Delete? (y/N): ");
        result
    }

    /// A reader that returns one line per `read` call.
    struct LineByLine {
        lines: Vec<&'static [u8]>,
    }

    impl Read for LineByLine {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.lines.is_empty() {
                return Ok(0);
            }
            let line = self.lines.remove(0);
            buf[..line.len()].copy_from_slice(line);
            Ok(line.len())
        }
    }

    struct Failing;

    impl Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("boom"))
        }
    }

    #[test]
    fn accepts_only_y_and_yes() {
        for yes in [
            &b"y\n"[..],
            b"Y\n",
            b"yes\n",
            b"YES\r\n",
            b"Yes\nno\n",
            // Go's ReadString returns the data it read before EOF.
            b"y",
            b"yes",
        ] {
            assert!(answer(yes), "{:?}", String::from_utf8_lossy(yes));
        }
        for no in [
            &b""[..],
            b"\n",
            b"n\n",
            b"no\n",
            b" y\n",
            b"y \n",
            b"yes\r\r\n",
            b"yess\n",
            b"\ny\n",
        ] {
            assert!(!answer(no), "{:?}", String::from_utf8_lossy(no));
        }
        let mut out = String::new();
        assert!(!confirm("Sure?", &mut Failing, &mut out));
        assert_eq!(out, "Sure? (y/N): ");
    }

    #[test]
    fn second_call_loses_buffered_input() {
        // The first call's buffer takes both lines; the second call sees EOF.
        let mut input = io::Cursor::new(b"n\ny\n".to_vec());
        let mut out = String::new();
        assert!(!confirm("First?", &mut input, &mut out));
        assert!(!confirm("Second?", &mut input, &mut out));
        assert_eq!(out, "First? (y/N): Second? (y/N): ");
        // When each read returns one line, the second call does get its line.
        let mut input = LineByLine {
            lines: vec![b"n\n", b"y\n"],
        };
        let mut out = String::new();
        assert!(!confirm("First?", &mut input, &mut out));
        assert!(confirm("Second?", &mut input, &mut out));
    }

    /// Records the size of each read it is asked for, then serves `data` in pieces of at
    /// most `piece` bytes.
    struct Recording {
        data: Vec<u8>,
        piece: usize,
        asked: Vec<usize>,
    }

    impl Read for Recording {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.asked.push(buf.len());
            let n = self.piece.min(buf.len()).min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data.drain(..n);
            Ok(n)
        }
    }

    /// Data and then an error: the line read so far is used, as Go ignores the error.
    struct DataThenError(Option<&'static [u8]>);

    impl Read for DataThenError {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.0.take() {
                Some(data) => {
                    buf[..data.len()].copy_from_slice(data);
                    Ok(data.len())
                }
                None => Err(io::Error::other("boom")),
            }
        }
    }

    #[test]
    fn reads_past_one_buffer_until_newline_or_end() {
        // A line longer than the 4096-byte buffer is read on to its newline.
        let mut data = vec![b'a'; 5000];
        data.extend_from_slice(b"\nyes\n");
        let mut input = Recording {
            data,
            piece: usize::MAX,
            asked: Vec::new(),
        };
        let mut out = String::new();
        assert!(!confirm("Long?", &mut input, &mut out));
        // The first read fills the buffer without a newline, the second reads on and
        // takes the rest, newline and the next line included (lost to the next call).
        assert_eq!(input.asked, vec![4096, 4096]);
        assert!(input.data.is_empty());
        // Pieces without a newline: each read asks for the room left in the buffer.
        let mut input = Recording {
            data: b"y".to_vec(),
            piece: 1,
            asked: Vec::new(),
        };
        assert!(confirm("Piece?", &mut input, &mut out));
        assert_eq!(input.asked, vec![4096, 4095]);
        // A read that takes the newline is the last one: nothing past it is requested.
        let mut input = Recording {
            data: b"n\ny\n".to_vec(),
            piece: 2,
            asked: Vec::new(),
        };
        assert!(!confirm("Newline?", &mut input, &mut out));
        assert_eq!(input.asked, vec![4096]);
        assert_eq!(input.data, b"y\n");
    }

    #[test]
    fn data_before_an_error_or_end_is_the_answer() {
        let mut out = String::new();
        assert!(confirm("Err?", &mut DataThenError(Some(b"yes")), &mut out));
        assert!(!confirm("Err?", &mut DataThenError(Some(b"no")), &mut out));
        assert!(!confirm("Err?", &mut DataThenError(Some(b"")), &mut out));
    }
}

//! The y/n confirmation prompt, called only by delete-domain and compact of s5-atlas-lib-3.
//! Owner: s5-atlas-lib-2.
//!
//! A port of the Go tool's `writeConfirm` (`delete.go:9-14`).

use std::io::Read;

/// `writeConfirm(prompt)`: writes `<prompt> (y/N): ` to `stdout`, reads one line through a
/// fresh 4096-byte buffer (what a new `bufio.Reader` takes from `input`), strips one
/// trailing `\n` then one `\r`, lowercases with Go's `strings.ToLower` and answers true
/// only for `y` or `yes`. Input with no data (EOF or a read error) is false. Input the
/// buffer took beyond the line is lost, so a second call does not see it (a suspected Go
/// defect, kept). The prompt goes to stdout even in JSON mode, as in Go.
pub fn confirm(prompt: &str, input: &mut dyn Read, stdout: &mut String) -> bool {
    // stub until t2
    let _ = (prompt, input, stdout);
    false
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
}

//! Zero-dependency ANSI styling and visible-width helpers.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Colors {
    pub enabled: bool,
}

impl Colors {
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    fn wrap(self, open: &str, close: &str, value: &str) -> String {
        if self.enabled {
            format!("\x1b[{open}m{value}\x1b[{close}m")
        } else {
            value.to_owned()
        }
    }

    pub fn bold(self, value: &str) -> String {
        self.wrap("1", "22", value)
    }

    pub fn dim(self, value: &str) -> String {
        self.wrap("2", "22", value)
    }

    pub fn red(self, value: &str) -> String {
        self.wrap("31", "39", value)
    }

    pub fn green(self, value: &str) -> String {
        self.wrap("32", "39", value)
    }

    pub fn yellow(self, value: &str) -> String {
        self.wrap("33", "39", value)
    }

    pub fn blue(self, value: &str) -> String {
        self.wrap("34", "39", value)
    }

    pub fn magenta(self, value: &str) -> String {
        self.wrap("35", "39", value)
    }

    pub fn cyan(self, value: &str) -> String {
        self.wrap("36", "39", value)
    }

    pub fn id(self, value: &str) -> String {
        self.wrap("38;5;179", "39", value)
    }
}

pub fn color_enabled(
    no_color: Option<&str>,
    force_color: Option<&str>,
    stdout_is_tty: bool,
) -> bool {
    if no_color.is_some_and(|value| !value.is_empty()) {
        return false;
    }
    if force_color.is_some_and(|value| !value.is_empty() && value != "0") {
        return true;
    }
    stdout_is_tty
}

pub fn strip_ansi(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'[') {
            let mut end = index + 2;
            while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b';') {
                end += 1;
            }
            if bytes.get(end) == Some(&b'm') {
                index = end + 1;
                continue;
            }
        }
        let character = value[index..].chars().next().expect("valid UTF-8 boundary");
        output.push(character);
        index += character.len_utf8();
    }
    output
}

pub fn visible_length(value: &str) -> usize {
    strip_ansi(value).chars().count()
}

pub fn pad_end(value: &str, width: usize) -> String {
    let length = visible_length(value);
    if length >= width {
        value.to_owned()
    } else {
        format!("{value}{}", " ".repeat(width - length))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honors_color_environment_precedence() {
        assert!(!color_enabled(Some("1"), Some("1"), true));
        assert!(color_enabled(None, Some("1"), false));
        assert!(!color_enabled(None, Some("0"), false));
    }

    #[test]
    fn removes_ansi_for_visible_width() {
        let colored = Colors::new(true).cyan("abc");
        assert_eq!(strip_ansi(&colored), "abc");
        assert_eq!(visible_length(&colored), 3);
        assert_eq!(pad_end(&colored, 5), format!("{colored}  "));
    }
}

use ansi_term::Colour::{Cyan, Green, Red, Yellow};

pub fn write_green(txt: &str) -> String {
    format!("{}", Green.paint(txt))
}

pub fn write_red(txt: &str) -> String {
    format!("{}", Red.paint(txt))
}

pub fn write_yellow(txt: &str) -> String {
    format!("{}", Yellow.paint(txt))
}

pub fn write_cyan(txt: &str) -> String {
    format!("{}", Cyan.paint(txt))
}

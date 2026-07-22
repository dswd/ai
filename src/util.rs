use std::cmp::max;

fn bar(len: usize) -> String {
    (0..max(len, 3)).map(|_| "=").collect::<String>()
}

pub fn bar_line() -> String {
    bar(80)
}

pub fn bar_title(title: &str) -> String {
    format!("{} {} {}", bar(10), title, bar(80 -12 -title.len()))
}
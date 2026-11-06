use std::fmt;

#[derive(Debug, Clone)]
pub struct Diag {
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub msg: String,
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}:{}: error: {}", self.file, self.line, self.col, self.msg)
    }
}

pub type R<T> = Result<T, Diag>;

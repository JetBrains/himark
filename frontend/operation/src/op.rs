#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Retain(u32),
    Insert(String),
    Delete(String),
}

impl Op {
    pub fn retain(len: u32) -> Self {
        Self::Retain(len)
    }

    pub fn insert(text: impl Into<String>) -> Self {
        Self::Insert(text.into())
    }

    pub fn delete(text: impl Into<String>) -> Self {
        Self::Delete(text.into())
    }

    pub fn old_len(&self) -> u32 {
        match self {
            Self::Retain(len) => *len,
            Self::Insert(_) => 0,
            Self::Delete(text) => byte_len(text),
        }
    }

    pub fn new_len(&self) -> u32 {
        match self {
            Self::Retain(len) => *len,
            Self::Insert(text) => byte_len(text),
            Self::Delete(_) => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.old_len() == 0 && self.new_len() == 0
    }
}

pub(crate) fn byte_len(text: &str) -> u32 {
    u32::try_from(text.len()).expect("text length does not fit into u32")
}

pub(crate) fn slice_bytes(text: &str, start: u32, len: u32) -> String {
    match len {
        0 => String::new(),
        _ => {
            let start = start as usize;
            let len = len as usize;
            let end = start + len;
            text.get(start..end)
                .expect("operation split must stay on UTF-8 boundaries")
                .to_owned()
        }
    }
}

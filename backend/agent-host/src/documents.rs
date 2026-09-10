use himark_ahp_ext_types::{text, DocumentApplied, Uid};
use himark_text::Text;
use serde_json::Value;

pub(crate) const CHANNEL_PREFIX: &str = "ahp-document:/";

#[derive(Clone)]
pub(crate) struct Document {
    text: Text,
    version: Uid,
}

impl Document {
    pub(crate) fn open(text: &str, version: Uid) -> Document {
        Document {
            text: Text::from_string_exact(text),
            version,
        }
    }

    pub(crate) fn version(&self) -> Uid {
        self.version
    }

    pub(crate) fn snapshot(&self, uri: Option<&str>) -> Value {
        let mut state = serde_json::json!({
            "text": text::materialize(&self.text),
            "version": self.version.to_string(),
        });
        if let Some(uri) = uri {
            state["uri"] = Value::String(uri.to_owned());
        }
        state
    }

    pub(crate) fn dispatch(&mut self, action: &DocumentApplied) -> bool {
        if action.base != self.version {
            return false;
        }
        let Ok(next) = text::apply(&self.text, &action.operation) else {
            return false;
        };
        self.text = next;
        self.version = action.id;
        true
    }
}

pub(crate) fn mint(seq: u64) -> Uid {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    Uid((nanos << 32) ^ u128::from(seq))
}

#[cfg(test)]
mod tests;

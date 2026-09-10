use std::sync::Arc;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ResourceType(Arc<str>);

impl ResourceType {
    pub const DOCUMENT: &'static str = "document";

    pub const DIRECTORY: &'static str = "dir";

    pub fn new(kind: impl Into<Arc<str>>) -> Self {
        Self(kind.into())
    }

    pub fn document() -> Self {
        Self::new(Self::DOCUMENT)
    }

    pub fn directory() -> Self {
        Self::new(Self::DIRECTORY)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_document(&self) -> bool {
        &*self.0 == Self::DOCUMENT
    }

    pub fn is_directory(&self) -> bool {
        &*self.0 == Self::DIRECTORY
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Authority(Arc<str>);

impl Authority {
    pub fn new(authority: impl Into<Arc<str>>) -> Self {
        Self(authority.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ResourceLocation {
    kind: ResourceType,
    authority: Authority,
    path: Arc<[String]>,
}

impl ResourceLocation {
    pub fn new(kind: ResourceType, authority: Authority, path: impl Into<Arc<[String]>>) -> Self {
        Self {
            kind,
            authority,
            path: path.into(),
        }
    }

    pub fn kind(&self) -> &ResourceType {
        &self.kind
    }

    pub fn is_synthetic(&self) -> bool {
        matches!(self.authority.as_str(), "scratch" | "demo")
    }

    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    pub fn path(&self) -> &[String] {
        &self.path
    }

    pub fn name(&self) -> &str {
        self.path
            .last()
            .map(String::as_str)
            .unwrap_or_else(|| self.authority.as_str())
    }

    pub fn extension(&self) -> String {
        self.name()
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_lowercase())
            .unwrap_or_default()
    }

    pub fn child(&self, kind: ResourceType, segment: impl Into<String>) -> Self {
        let mut path: Vec<String> = self.path.to_vec();
        path.push(segment.into());
        Self {
            kind,
            authority: self.authority.clone(),
            path: path.into(),
        }
    }
}

#[cfg(test)]
mod tests;

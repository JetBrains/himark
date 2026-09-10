use himark::higent::ResourceUri;
use himark::{Authority, ResourceLocation, ResourceType};

pub struct FileUris;

impl himark::higent::ResourceUriMap for FileUris {
    fn uri_of(&self, location: &ResourceLocation) -> ResourceUri {
        ResourceUri::new(format!("file:///{}", location.path().join("/")))
    }

    fn location_of(
        &self,
        uri: &ResourceUri,
        kind: ResourceType,
        authority: &Authority,
    ) -> Option<ResourceLocation> {
        let path = uri.as_str().strip_prefix("file://")?;
        let decoded = percent_decode(path);
        let segments: Vec<String> = decoded
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();
        Some(ResourceLocation::new(kind, authority.clone(), segments))
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if let (Some(high), Some(low)) = (
                bytes.get(index + 1).and_then(|b| (*b as char).to_digit(16)),
                bytes.get(index + 2).and_then(|b| (*b as char).to_digit(16)),
            ) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

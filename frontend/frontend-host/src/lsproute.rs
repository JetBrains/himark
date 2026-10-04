// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use ahp_wire::fs::ClientDirectory;
use hicode::{CodeTarget, FindDefinitionEffect};
use documents::text_ext::LineCol;
use editor::location::ResourceLocation;
use imba::effect::EffectHandler;
use serde_json::{json, Value};

pub(crate) struct DefinitionRoute {
    pub(crate) directory: Arc<ClientDirectory>,
    pub(crate) uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
}

impl EffectHandler<FindDefinitionEffect> for DefinitionRoute {
    async fn handle(&self, effect: FindDefinitionEffect) -> Option<Vec<CodeTarget>> {
        locate(
            &self.directory,
            &*self.uris,
            &effect.location,
            effect.position,
            "textDocument/definition",
        )
        .await
    }
}

async fn locate(
    directory: &ClientDirectory,
    uris: &dyn ahp_wire::client::ResourceUriMap,
    location: &ResourceLocation,
    position: LineCol,
    method: &str,
) -> Option<Vec<CodeTarget>> {
    let (client, session) = ahp_wire::fs::client_of(directory, location)?;
    let uri = uris.uri_of(location).into_string();
    let params = json!({
        "textDocument": { "uri": uri },
        "position": { "line": position.line, "character": position.col },
    });
    let result = client.lsp.lsp(session, method.to_owned(), params).await.ok()?;
    Some(parse_targets(&result, uris, location))
}

fn parse_targets(
    result: &Value,
    uris: &dyn ahp_wire::client::ResourceUriMap,
    asked: &ResourceLocation,
) -> Vec<CodeTarget> {
    match result {
        Value::Null => Vec::new(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| target_of(item, uris, asked))
            .collect(),
        single => target_of(single, uris, asked).into_iter().collect(),
    }
}

fn target_of(
    value: &Value,
    uris: &dyn ahp_wire::client::ResourceUriMap,
    asked: &ResourceLocation,
) -> Option<CodeTarget> {
    let (uri, range) = match value.get("uri") {
        Some(uri) => (uri, value.get("range")?),
        None => (
            value.get("targetUri")?,
            value
                .get("targetSelectionRange")
                .or_else(|| value.get("targetRange"))?,
        ),
    };
    let position = |point: &Value| -> Option<LineCol> {
        Some(LineCol {
            line: point.get("line")?.as_u64()? as u32,
            col: point.get("character")?.as_u64()? as u32,
        })
    };
    Some(CodeTarget {
        location: uris.location_of(
            &ahp_wire::client::ResourceUri::new(uri.as_str()?),
            editor::location::ResourceType::document(),
            asked.authority(),
        )?,
        range: position(range.get("start")?)?..position(range.get("end")?)?,
    })
}

#[cfg(test)]
mod tests;

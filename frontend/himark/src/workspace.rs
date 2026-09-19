// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::effect::{AnyEffect, Effect};
use imba::store::Store;

use crate::app::{AppCommand, AppFx};
use crate::ResourceLocation;

pub struct StoreDocumentEffect {
    pub location: ResourceLocation,
    pub text: String,
}

impl Effect for StoreDocumentEffect {
    type Result = bool;
}

pub struct ListDirectoryEffect {
    pub location: ResourceLocation,
}

impl Effect for ListDirectoryEffect {
    type Result = Option<Vec<ResourceLocation>>;
}

pub struct PickSaveEffect {
    pub suggested: String,
}

impl Effect for PickSaveEffect {
    type Result = Option<ResourceLocation>;
}

pub struct BuildDocumentEffect {
    pub location: ResourceLocation,
    pub text: String,
    pub prep: Option<RowPrep>,
}

impl Effect for BuildDocumentEffect {
    type Result = BuiltDocument;
}

pub struct RowPrep {
    pub spans: crate::SpanSource,

    pub width: f32,
}

pub struct BuiltDocument {
    pub document: crate::Document,
    pub spans: Option<crate::GroupSpans>,
    pub prebuilt: Option<crate::PrebuiltRows>,
}

pub fn prepare_built(
    location: &ResourceLocation,
    document: crate::Document,
    prep: Option<&RowPrep>,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &crate::Theme,
) -> BuiltDocument {
    let Some(prep) = prep else {
        return BuiltDocument {
            document,
            spans: None,
            prebuilt: None,
        };
    };
    let spans = (prep.spans)(location, &document);
    let prebuilt = prebuild_group(&document, &spans, prep.width, fonts, theme);
    BuiltDocument {
        document,
        spans: Some(spans),
        prebuilt: Some(prebuilt),
    }
}

pub fn prebuild_group(
    document: &crate::Document,
    spans: &crate::GroupSpans,
    width: f32,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &crate::Theme,
) -> crate::PrebuiltRows {
    let revision = document.revision();
    let markup_generation = document.markup_generation();

    let mut composed = document.clone();
    let tint = composed.add_markup();
    let mut markup = crate::Markup::new();
    for range in &spans.marks {
        markup.push_styled(range.clone(), crate::StyleId::Match);
    }
    composed.replace_markup(
        tint,
        markup,
        &[],
        fonts,
        theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let snapped = crate::snap_ranges(&composed, &spans.ranges);
    let rows = composed.prebuild_row_layouts(&[tint], &snapped, width, fonts, theme);
    crate::PrebuiltRows {
        revision,
        markup_generation,
        rows,
    }
}

pub struct OpenByLocationEffect {
    pub window: crate::WindowId,
    pub location: ResourceLocation,
    pub primary: bool,

    pub target: Option<std::ops::Range<crate::LineCol>>,
}

impl Effect for OpenByLocationEffect {
    type Result = AppCommand;
}

pub struct OpenDiffByLocationsEffect {
    pub window: crate::WindowId,
    pub old: ResourceLocation,
    pub new: ResourceLocation,
}

impl Effect for OpenDiffByLocationsEffect {
    type Result = AppCommand;
}

/// One diff-canvas item's whole off-thread half (docs/editor/diff-canvas.md
/// §4): fetch both sides, build language-aware documents, diff,
/// prepare the marks. The landing only mounts.
pub struct BuildFileDiffEffect {
    pub old: ResourceLocation,
    pub new: ResourceLocation,

    /// The canvas's content width at arm time — the landing lays the
    /// editors at it and re-arms if the panel resized meanwhile.
    pub width: f32,
}

pub struct BuiltFileDiff {
    pub old: crate::Document,
    pub new: crate::Document,
    pub operation: crate::Operation,
    pub marks: crate::PreparedMarks,
    pub width: f32,

    /// Both sides unreachable — the row reports instead of mounting.
    pub failed: Option<String>,
}

impl Effect for BuildFileDiffEffect {
    type Result = BuiltFileDiff;
}

pub fn open_by_location_effect(
    window: crate::WindowId,
    location: ResourceLocation,
    primary: bool,
    target: Option<std::ops::Range<crate::LineCol>>,
) -> crate::AppEffect {
    AnyEffect::new(OpenByLocationEffect {
        window,
        location,
        primary,
        target,
    })
}

pub fn open_locations(
    store: &mut Store,
    window: crate::WindowId,
    locations: &[ResourceLocation],
    fx: &mut AppFx<'_>,
) {
    let folders = crate::Windows::window_ref(store, window)
        .map(|entity| crate::higent::session_folders(store, &entity.current_session()))
        .unwrap_or_default();
    let locations: Vec<ResourceLocation> = locations
        .iter()
        .map(|location| {
            if location.authority().as_str() != "local" {
                return location.clone();
            }
            match folders
                .iter()
                .find(|folder| location.path().starts_with(folder.path()))
            {
                Some(folder) => ResourceLocation::new(
                    location.kind().clone(),
                    folder.authority().clone(),
                    location.path().to_vec(),
                ),
                None => location.clone(),
            }
        })
        .collect();
    let mut primary = true;
    for location in &locations {
        if let Some(document) = crate::OpenDocuments::by_location(store, location) {
            if primary {
                if let Some(mut window_entity) = crate::Windows::window(store, window) {
                    window_entity.show_document(store, window, document, None, fx);
                    crate::Windows::put(store, window, window_entity);
                }
            }
            primary = false;
            continue;
        }
        fx.push(open_by_location_effect(
            window,
            location.clone(),
            primary,
            None,
        ));
        primary = false;
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SessionId {
    pub host: crate::higent::HostId,

    pub session: String,
}

impl SessionId {
    pub fn local_default(store: &Store) -> SessionId {
        let host = store
            .get::<crate::higent::LocalHost>()
            .and_then(|local| local.0)
            .unwrap_or(crate::higent::HostId::LOCAL);
        SessionId {
            host,
            session: host_discovery::LOCAL_FS_SESSION.to_owned(),
        }
    }

    pub fn mint_scratch(store: &mut Store) -> SessionId {
        let mut minted = 0;
        store.update::<ScratchSpaces>(|spaces| {
            spaces.0 += 1;
            minted = spaces.0;
        });
        SessionId {
            host: Self::local_default(store).host,
            session: format!("scratch-space:{minted}"),
        }
    }

    pub fn names_session(&self) -> bool {
        self.session != host_discovery::LOCAL_FS_SESSION
            && !self.session.starts_with("scratch-space:")
    }
}

#[derive(Clone, Default)]
pub struct ScratchSpaces(u64);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FindTarget {
    Text,

    Path,
}

pub struct FindEffect {
    pub folders: Vec<ResourceLocation>,
    pub term: String,
    pub target: FindTarget,
}

impl Effect for FindEffect {
    type Result = Vec<ResourceLocation>;
}

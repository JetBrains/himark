// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use documents::sync::SyncEdit;
use editor::{EditIdentity, EditLog};
use operation::{Op, Operation};
use rebase::Offer;

use super::SyncState;

pub(crate) fn seam_edit(
    log: &EditLog,
    base_revision: u64,
    applied: Option<EditIdentity>,
) -> Option<SyncEdit> {
    let head = log.head()?;
    if Some(head) == applied {
        return None;
    }
    let op = log.compose_since(base_revision)?;
    Some(SyncEdit::captured(log.as_of(base_revision), op, head))
}

pub(crate) fn sent(revision: u64, attached_at: u64, taken: u64) -> u64 {
    revision.saturating_sub(attached_at).saturating_sub(taken)
}

pub(crate) fn offer_landing(
    document: &EditLog,
    revision: u64,
    attached_at: u64,
    taken: u64,
    shown: u32,
    offer: &Offer<SyncState>,
) -> Option<(EditIdentity, Operation)> {
    if offer.seen_local != sent(revision, attached_at, taken) {
        return None;
    }
    let slice = offer.state.slice_from(document)?;
    if is_identity(&slice) {
        return None;
    }
    if slice.old_len() != shown {
        return None;
    }
    Some((offer.state.log.head()?, slice))
}

pub(crate) fn is_identity(operation: &Operation) -> bool {
    operation.is_empty() || operation.iter().all(|op| matches!(op, Op::Retain(_)))
}

#[cfg(test)]
mod tests;

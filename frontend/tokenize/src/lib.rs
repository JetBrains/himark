use rope::{Cursor, Measure, MetricId, Rope, SeekMode};

pub trait Safepoint<T> {
    fn is_safepoint(&self, element: &T) -> bool;
    fn cut(&self, element: T, offset: u32) -> T;

    fn can_resume(&self, _element: &T, _alignment: u32) -> bool {
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RewriteReport<M> {
    pub old_elements_replaced: u32,
    pub replacement_elements: u32,
    pub stopped: bool,
    pub location: M,
}

pub fn rewrite<T, M, Tokens, Policy, Stop>(
    cursor: &mut Cursor<T, M>,
    tokens: Tokens,
    policy: &Policy,
    alignment_metric: MetricId,
    mut stop: Stop,
) -> RewriteReport<M::Metrics>
where
    T: Clone,
    M: Measure<T>,
    Tokens: IntoIterator<Item = T>,
    Policy: Safepoint<T>,
    Stop: FnMut(M::Metrics) -> bool,
{
    let mut location = cursor.position();
    let tokens = tokens.into_iter();
    let (lower_bound, _) = tokens.size_hint();
    let mut replacement = Vec::with_capacity(lower_bound);
    let mut stop_cursor = cursor.clone();
    let mut stopped = false;

    for token in tokens {
        if policy.is_safepoint(&token)
            && stop(location)
            && is_aligned_safepoint(&mut stop_cursor, policy, alignment_metric, location)
        {
            stopped = true;
            break;
        }

        M::add_assign(&mut location, M::measure(&token));
        replacement.push(token);
    }

    let stop_alignment = M::metric_at(&location, alignment_metric);
    let mut old_cursor = cursor.clone();
    let mut old_elements_replaced = 0;

    while old_cursor.size() != 0 {
        let old_start = M::metric_at(&old_cursor.position(), alignment_metric);
        let old_metrics = old_cursor.element_metrics();
        let old_width = M::metric_at(&old_metrics, alignment_metric);
        if stop_alignment <= old_start {
            if stopped || old_width != 0 || old_start != stop_alignment {
                break;
            }
            old_elements_replaced += 1;
            if !old_cursor.advance() {
                break;
            }
            continue;
        }

        let old_end = old_start.saturating_add(old_width);

        if stop_alignment < old_end {
            let offset = stop_alignment - old_start;
            replacement.push(policy.cut(old_cursor.element().clone(), offset));
            old_elements_replaced += 1;
            break;
        }

        old_elements_replaced += 1;
        if !old_cursor.advance() {
            break;
        }
    }

    let replacement_elements =
        u32::try_from(replacement.len()).expect("replacement element count exceeds u32");
    cursor.delete(old_elements_replaced);

    if !replacement.is_empty() {
        cursor.insert(Rope::<T, M>::from_iter(replacement));
    }

    RewriteReport {
        old_elements_replaced,
        replacement_elements,
        stopped,
        location,
    }
}

fn is_aligned_safepoint<T, M, Policy>(
    cursor: &mut Cursor<T, M>,
    policy: &Policy,
    alignment_metric: MetricId,
    location: M::Metrics,
) -> bool
where
    T: Clone,
    M: Measure<T>,
    Policy: Safepoint<T>,
{
    let alignment = M::metric_at(&location, alignment_metric);
    if !cursor.seek(alignment_metric, alignment, SeekMode::After) {
        return false;
    }

    M::metric_at(&cursor.position(), alignment_metric) == alignment
        && policy.is_safepoint(cursor.element())
        && policy.can_resume(cursor.element(), alignment)
}

#[cfg(test)]
mod tests;

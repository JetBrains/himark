use rope::{Measure, Metrics};

use crate::op::Op;

pub(crate) const OP_METRIC_RANK: usize = 2;
pub(crate) const OLD_LEN: usize = 0;
pub(crate) const NEW_LEN: usize = 1;
pub(crate) const LEAF_CAPACITY: usize = 32;
pub(crate) const BRANCH_FACTOR: usize = 16;

#[derive(Clone, Copy, Debug)]
pub(crate) struct OperationMeasure;

impl Measure<Op> for OperationMeasure {
    type Metrics = Metrics<OP_METRIC_RANK>;

    fn zero() -> Self::Metrics {
        Metrics::zero()
    }

    fn metric_at(metrics: &Self::Metrics, id: rope::MetricId) -> u32 {
        metrics.metric_at(id)
    }

    fn add_assign(metrics: &mut Self::Metrics, other: Self::Metrics) {
        metrics.add_assign(other);
    }

    fn sub_assign(metrics: &mut Self::Metrics, other: Self::Metrics) {
        metrics.sub_assign(other);
    }

    fn measure(op: &Op) -> Self::Metrics {
        Metrics([op.old_len(), op.new_len()])
    }
}

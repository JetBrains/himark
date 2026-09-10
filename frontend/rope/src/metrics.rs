#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics<const RANK: usize>(pub [u32; RANK]);

impl<const RANK: usize> Metrics<RANK> {
    pub const fn zero() -> Self {
        Self([0; RANK])
    }

    pub fn metric_at(&self, id: MetricId) -> u32 {
        assert!(id.0 < RANK, "metric id is out of bounds");
        self.0[id.0]
    }

    pub fn add_assign(&mut self, other: Self) {
        let mut i = 0;
        while i < RANK {
            self.0[i] += other.0[i];
            i += 1;
        }
    }

    pub fn sub_assign(&mut self, other: Self) {
        let mut i = 0;
        while i < RANK {
            self.0[i] -= other.0[i];
            i += 1;
        }
    }
}

pub trait Measure<T: Clone> {
    type Metrics: Clone + Copy + std::fmt::Debug + PartialEq + Eq;

    fn zero() -> Self::Metrics;

    fn metric_at(metrics: &Self::Metrics, id: MetricId) -> u32;

    fn add_assign(metrics: &mut Self::Metrics, other: Self::Metrics);

    fn sub_assign(metrics: &mut Self::Metrics, other: Self::Metrics);

    fn measure(t: &T) -> Self::Metrics;
}

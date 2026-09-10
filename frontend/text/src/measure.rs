use rope::{Measure, MetricId, Metrics};

pub(crate) const TEXT_RANK: usize = 2;
pub(crate) const NEWLINES: MetricId = MetricId(0);
pub(crate) const UTF16: MetricId = MetricId(1);
pub(crate) const LEAF_CAPACITY: usize = 1024;
pub(crate) const BRANCH_FACTOR: usize = 16;

#[derive(Clone, Copy, Debug)]
pub(crate) struct TextMeasure;

impl Measure<u8> for TextMeasure {
    type Metrics = Metrics<TEXT_RANK>;

    fn zero() -> Self::Metrics {
        Metrics::zero()
    }

    fn metric_at(metrics: &Self::Metrics, id: MetricId) -> u32 {
        metrics.metric_at(id)
    }

    fn add_assign(metrics: &mut Self::Metrics, other: Self::Metrics) {
        metrics.add_assign(other);
    }

    fn sub_assign(metrics: &mut Self::Metrics, other: Self::Metrics) {
        metrics.sub_assign(other);
    }

    fn measure(byte: &u8) -> Self::Metrics {
        Metrics([u32::from(*byte == b'\n'), utf16_units(*byte)])
    }
}

pub(crate) fn utf16_units(byte: u8) -> u32 {
    match byte {
        0x00..=0x7F => 1,
        0x80..=0xBF => 0,
        0xC0..=0xEF => 1,
        _ => 2,
    }
}

pub(crate) fn is_continuation_byte(byte: u8) -> bool {
    (byte & 0b1100_0000) == 0b1000_0000
}

pub(crate) fn split_utf8_bytes(bytes: &[u8], leaf_capacity: usize) -> Vec<Vec<u8>> {
    match bytes.is_empty() {
        true => Vec::new(),
        false => {
            let mut leaves = Vec::new();
            let mut start = 0usize;
            while start < bytes.len() {
                let mut end = (start + leaf_capacity).min(bytes.len());
                while end < bytes.len() && is_continuation_byte(bytes[end]) {
                    end -= 1;
                }
                if end == start {
                    end = bytes.len();
                    while end > start && is_continuation_byte(bytes[end - 1]) {
                        end -= 1;
                    }
                    if end == start {
                        panic!("invalid UTF-8 boundary while splitting text leaves");
                    }
                }
                leaves.push(bytes[start..end].to_vec());
                start = end;
            }
            leaves
        }
    }
}

pub(crate) fn count_newlines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&byte| byte == b'\n').count()
}

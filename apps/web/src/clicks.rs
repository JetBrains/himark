#[derive(Default)]
pub(super) struct ClickCounter {
    last_ms: Option<f64>,
    point: (f32, f32),
    count: u8,
}

impl ClickCounter {
    pub(super) fn count(&mut self, timestamp_ms: f64, x: f32, y: f32, button: u16) -> Option<u8> {
        if button != 0 {
            *self = Self::default();
            return None;
        }
        let run = self.last_ms.is_some_and(|last| {
            (0.0..500.0).contains(&(timestamp_ms - last))
                && (x - self.point.0).abs() <= 6.0
                && (y - self.point.1).abs() <= 6.0
        });
        self.count = if run { (self.count + 1).min(3) } else { 1 };
        self.last_ms = Some(timestamp_ms);
        self.point = (x, y);
        Some(self.count)
    }
}

#[cfg(test)]
mod tests {
    use super::ClickCounter;

    #[test]
    fn repeated_presses_select_words_then_lines() {
        let mut clicks = ClickCounter::default();
        assert_eq!(clicks.count(0.0, 10.0, 20.0, 0), Some(1));
        assert_eq!(clicks.count(100.0, 12.0, 19.0, 0), Some(2));
        assert_eq!(clicks.count(200.0, 10.0, 20.0, 0), Some(3));
        assert_eq!(clicks.count(300.0, 10.0, 20.0, 0), Some(3));
    }

    #[test]
    fn time_or_distance_starts_a_new_run() {
        let mut clicks = ClickCounter::default();
        assert_eq!(clicks.count(0.0, 10.0, 20.0, 0), Some(1));
        assert_eq!(clicks.count(499.0, 16.0, 26.0, 0), Some(2));
        assert_eq!(clicks.count(999.0, 16.0, 26.0, 0), Some(1));
        assert_eq!(clicks.count(1000.0, 23.0, 26.0, 0), Some(1));
        assert_eq!(clicks.count(1001.0, 23.0, 33.0, 0), Some(1));
        assert_eq!(clicks.count(900.0, 23.0, 33.0, 0), Some(1));
    }

    #[test]
    fn other_buttons_interrupt_selection_clicks() {
        for button in [1, 2] {
            let mut clicks = ClickCounter::default();
            assert_eq!(clicks.count(0.0, 10.0, 20.0, 0), Some(1));
            assert_eq!(clicks.count(100.0, 10.0, 20.0, button), None);
            assert_eq!(clicks.count(200.0, 10.0, 20.0, 0), Some(1));
        }
    }
}

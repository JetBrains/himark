use super::*;

fn clock(ms: f64) -> AnimationClock {
    AnimationClock::from_millis(ms)
}

#[test]
fn advances_by_elapsed_time_and_settles() {
    let mut size = Animation::done(
        10.0f32,
        Motion::Ease {
            duration_ms: 100.0,
            easing: Easing::Linear,
        },
    );
    assert!(!size.running());

    size.set(20.0);
    assert!(size.running());
    assert_eq!(size.value(), 10.0);

    size.advance(clock(1000.0));
    assert_eq!(size.value(), 10.0);

    size.advance(clock(1050.0));
    assert_eq!(size.value(), 15.0);

    size.advance(clock(1500.0));
    assert_eq!(size.value(), 20.0);
    assert!(!size.running(), "a finished animation is silent");
}

#[test]
fn retargeting_redirects_from_the_displayed_value() {
    let mut size = Animation::done(
        0.0f32,
        Motion::Ease {
            duration_ms: 100.0,
            easing: Easing::Linear,
        },
    );
    size.set(100.0);
    size.advance(clock(0.0));
    size.advance(clock(50.0));
    assert_eq!(size.value(), 50.0);

    size.set(0.0);
    size.advance(clock(60.0));
    size.advance(clock(110.0));
    assert_eq!(size.value(), 25.0);

    size.advance(clock(160.0));
    assert_eq!(size.value(), 0.0);
    assert!(!size.running());
}

#[test]
fn products_animate_as_one_value() {
    let mut size = Animation::done(
        (0.0f32, 100.0f32),
        Motion::Ease {
            duration_ms: 100.0,
            easing: Easing::Linear,
        },
    );
    size.set((10.0, 200.0));
    size.advance(clock(0.0));
    size.advance(clock(50.0));
    assert_eq!(size.value(), (5.0, 150.0));
}

#[test]
fn setting_the_current_target_stays_silent() {
    let mut size = Animation::done(
        7.0f32,
        Motion::Ease {
            duration_ms: 100.0,
            easing: Easing::EaseInOut,
        },
    );
    size.set(7.0);
    assert!(!size.running(), "no-op retargets must not wake the clock");
}

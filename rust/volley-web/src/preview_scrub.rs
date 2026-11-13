//! Pure, deterministic keyboard-scrub policy for the browser's video player.
//! DOM events and HTMLVideoElement.currentTime stay in the JS platform adapter.

const TAP_SECONDS: f64 = 0.1;
const HOLD_DELAY_MS: f64 = 280.0;
const HOLD_DOUBLING_MS: f64 = 650.0;
const INITIAL_SECONDS_PER_SECOND: f64 = 2.5;
const MAX_SECONDS_PER_SECOND: f64 = 80.0;
const MAX_FRAME_MS: f64 = 200.0;

fn signed_direction(direction: i32) -> f64 {
    match direction {
        -1 => -1.0,
        1 => 1.0,
        _ => 0.0,
    }
}

/// One discrete arrow-key press: exactly one tenth of a second in either direction.
#[no_mangle]
pub extern "C" fn volley_preview_seek_tap(direction: i32) -> f64 {
    signed_direction(direction) * TAP_SECONDS
}

/// Source seconds to advance for this rendered frame while holding an arrow.
/// Returns zero during the initial grace period. Frame latency and velocity are
/// bounded, so a suspended browser tab cannot unexpectedly jump many minutes.
#[no_mangle]
pub extern "C" fn volley_preview_seek_hold_delta(
    direction: i32,
    held_ms: f64,
    frame_ms: f64,
) -> f64 {
    let signed = signed_direction(direction);
    if signed == 0.0
        || !held_ms.is_finite()
        || !frame_ms.is_finite()
        || held_ms < HOLD_DELAY_MS
        || frame_ms <= 0.0
    {
        return 0.0;
    }
    let doublings = (held_ms - HOLD_DELAY_MS) / HOLD_DOUBLING_MS;
    let velocity =
        (INITIAL_SECONDS_PER_SECOND * 2.0_f64.powf(doublings)).min(MAX_SECONDS_PER_SECOND);
    signed * velocity * frame_ms.min(MAX_FRAME_MS) / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_is_always_one_tenth_of_a_second() {
        assert_eq!(volley_preview_seek_tap(-1), -0.1);
        assert_eq!(volley_preview_seek_tap(1), 0.1);
        assert_eq!(volley_preview_seek_tap(0), 0.0);
        assert_eq!(volley_preview_seek_tap(7), 0.0);
    }

    #[test]
    fn hold_starts_only_after_grace_period() {
        for held in [0.0, 16.0, 279.99] {
            assert_eq!(volley_preview_seek_hold_delta(1, held, 100.0), 0.0);
        }
        assert_eq!(volley_preview_seek_hold_delta(1, 280.0, 100.0), 0.25);
        assert_eq!(volley_preview_seek_hold_delta(-1, 280.0, 100.0), -0.25);
    }

    #[test]
    fn holding_accelerates_exponentially_and_stops_at_limit() {
        let t0 = volley_preview_seek_hold_delta(1, 280.0, 100.0);
        let t1 = volley_preview_seek_hold_delta(1, 930.0, 100.0);
        let t2 = volley_preview_seek_hold_delta(1, 1580.0, 100.0);
        assert!((t1 - 2.0 * t0).abs() < 1e-10);
        assert!((t2 - 4.0 * t0).abs() < 1e-10);
        assert_eq!(volley_preview_seek_hold_delta(1, 30_000.0, 100.0), 8.0);
        assert_eq!(volley_preview_seek_hold_delta(1, 30_000.0, 10_000.0), 16.0);
    }

    #[test]
    fn invalid_inputs_and_background_frame_stalls_are_safe() {
        for held in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(volley_preview_seek_hold_delta(1, held, 100.0), 0.0);
        }
        for frame in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
            assert_eq!(volley_preview_seek_hold_delta(1, 500.0, frame), 0.0);
        }
        assert_eq!(volley_preview_seek_hold_delta(2, 500.0, 100.0), 0.0);
        assert_eq!(volley_preview_seek_hold_delta(-1, 30_000.0, 500.0), -16.0);
    }
}

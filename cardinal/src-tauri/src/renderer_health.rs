#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_stalled_renderer_but_limits_reload_loops() {
        let mut health = Health::default();
        assert!(!health.check(0, true, 0));
        assert!(!health.check(30, true, 0));
        assert!(health.check(60, true, 0));
        assert!(!health.check(90, true, 0));
        assert!(health.check(120, true, 0));
        assert!(health.check(180, true, 0));
        assert!(!health.check(240, true, 0));
    }

    #[test]
    fn hidden_windows_and_fresh_responses_do_not_reload() {
        let mut health = Health::default();
        assert!(!health.check(0, false, 0));
        assert!(!health.check(500, false, 0));
        assert!(!health.check(501, true, 0));
        assert!(!health.check(540, true, 535));
        assert!(!health.check(560, true, 555));
    }
}

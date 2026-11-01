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
#[derive(Default)]
pub(crate) struct Health {
    visible_since: Option<u64>,
    last_reload: u64,
    reloads: Vec<u64>,
}

impl Health {
    pub(crate) fn check(&mut self, now: u64, visible: bool, last_response: u64) -> bool {
        if !visible {
            self.visible_since = None;
            return false;
        }
        let visible_since = *self.visible_since.get_or_insert(now);
        self.reloads.retain(|time| now.saturating_sub(*time) < 600);
        if now.saturating_sub(visible_since.max(last_response).max(self.last_reload)) < 60
            || self.reloads.len() >= 3
        {
            return false;
        }
        self.last_reload = now;
        self.reloads.push(now);
        true
    }
}

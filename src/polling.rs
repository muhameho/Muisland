//! Per-account quota scheduling. Account changes never bypass a cooldown.
pub const ACTIVE_SECONDS: f32 = 30.;
pub const OTHER_SECONDS: f32 = 120.;
pub const IDENTITY_SECONDS: f32 = 5.;
#[derive(Clone, Default)]
pub struct PollState {
    next_at: f32,
    last_attempt: Option<f32>,
    failures: u32,
}
impl PollState {
    pub fn due(&self, now: f32, active: bool) -> bool {
        if now >= self.next_at {
            return true;
        }
        self.failures == 0
            && active
            && self
                .last_attempt
                .is_some_and(|last| now - last >= ACTIVE_SECONDS)
    }
    pub fn success(&mut self, now: f32, active: bool) {
        self.last_attempt = Some(now);
        self.failures = 0;
        self.next_at = now
            + if active {
                ACTIVE_SECONDS
            } else {
                OTHER_SECONDS
            };
    }
    pub fn failure(
        &mut self,
        now: f32,
        active: bool,
        retry_after: Option<u64>,
        auth_required: bool,
    ) {
        self.last_attempt = Some(now);
        self.failures = self.failures.saturating_add(1);
        let base = if active {
            ACTIVE_SECONDS
        } else {
            OTHER_SECONDS
        };
        let backoff = (base * 2f32.powi(self.failures.min(5) as i32)).min(900.);
        self.next_at = now
            + backoff
                .max(retry_after.unwrap_or(0) as f32)
                .max(if auth_required { 900. } else { 0. });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_refreshes_in_thirty_seconds_and_other_accounts_in_two_minutes() {
        let mut p = PollState::default();
        assert!(p.due(0., false));
        p.success(10., true);
        assert!(!p.due(39., true));
        assert!(p.due(40., true));
        p.success(40., false);
        assert!(!p.due(159., false));
        assert!(p.due(160., false));
        assert!(p.due(70., true));
    }
    #[test]
    fn rate_limit_cannot_be_bypassed_by_switching_to_the_account() {
        let mut p = PollState::default();
        p.failure(10., false, Some(600), false);
        assert!(!p.due(300., true));
        assert!(!p.due(609., false));
        assert!(p.due(610., true));
    }
    #[test]
    fn errors_back_off_and_success_restores_normal_cadence() {
        let mut p = PollState::default();
        p.failure(0., true, None, false);
        assert!(!p.due(59., true));
        assert!(p.due(60., true));
        p.failure(60., true, None, false);
        assert!(!p.due(179., true));
        p.success(180., true);
        assert!(p.due(210., true));
    }
    #[test]
    fn authentication_failure_does_not_cause_a_retry_loop() {
        let mut p = PollState::default();
        p.failure(0., true, None, true);
        assert!(!p.due(899., true));
        assert!(p.due(900., true));
    }
}

/// A failed presentation may recreate the context once. A second failure stays
/// dormant until a confirmed presentation or an explicit screen wake.
#[derive(Default)]
pub struct Recovery {
    attempted: bool,
}

impl Recovery {
    pub fn failed(&mut self) -> bool {
        !std::mem::replace(&mut self.attempted, true)
    }

    pub fn presented(&mut self) {
        self.attempted = false;
    }

    pub fn wake(&mut self) {
        self.attempted = false;
    }
}

#[cfg(test)]
mod tests {
    use super::Recovery;

    #[test]
    fn persistent_failure_stops_after_one_recreation() {
        let mut recovery = Recovery::default();
        assert!(recovery.failed());
        for _ in 0..1000 {
            assert!(!recovery.failed());
        }
    }

    #[test]
    fn presentation_and_explicit_wake_allow_a_new_attempt() {
        let mut recovery = Recovery::default();
        assert!(recovery.failed());
        assert!(!recovery.failed());
        recovery.wake();
        assert!(recovery.failed());
        assert!(!recovery.failed());
        recovery.presented();
        assert!(recovery.failed());
        assert!(!recovery.failed());
    }
}

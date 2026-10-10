use super::collision::PortCollision;
use crate::LeptosTestAppConfig;

/// Maximum number of times we restart cargo-leptos on a port-bind collision before giving up.
///
/// Only collisions on an auto-allocated port are retried. If the user pinned the colliding port,
/// that's a configuration error, not a race, and we don't retry it.
const MAX_PORT_COLLISION_RETRIES: u32 = 3;

/// Port collisions that abort the current startup attempt so it can be retried with fresh ports.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy {
    site: bool,
    reload: bool,
}

impl RetryPolicy {
    /// The first attempt plus up to [`MAX_PORT_COLLISION_RETRIES`] retries.
    pub(crate) const MAX_ATTEMPTS: u32 = MAX_PORT_COLLISION_RETRIES + 1;

    /// The policy for the 1-based `attempt`. The final attempt retries nothing.
    pub(crate) fn for_attempt(config: &LeptosTestAppConfig, attempt: u32) -> Self {
        let can_retry = attempt < Self::MAX_ATTEMPTS;
        Self {
            site: can_retry && config.site_addr.is_none(),
            reload: can_retry && config.reload_port.is_none(),
        }
    }

    pub(crate) fn retries(self, collision: PortCollision) -> bool {
        match collision {
            PortCollision::App => self.site,
            PortCollision::Reload => self.reload,
        }
    }
}

#[cfg(test)]
mod tests {
    use assertr::prelude::*;

    use super::{PortCollision, RetryPolicy};
    use crate::LeptosTestAppConfig;

    #[test]
    fn retries_collisions_on_auto_allocated_ports() {
        let config = LeptosTestAppConfig::new("./test-app");
        let retry = RetryPolicy::for_attempt(&config, 1);

        assert_that!(retry.retries(PortCollision::App)).is_true();
        assert_that!(retry.retries(PortCollision::Reload)).is_true();
    }

    #[test]
    fn does_not_retry_collisions_on_pinned_ports() {
        let site_pinned = LeptosTestAppConfig::new("./test-app").with_site_addr("127.0.0.1:3000");
        let retry = RetryPolicy::for_attempt(&site_pinned, 1);
        assert_that!(retry.retries(PortCollision::App)).is_false();
        assert_that!(retry.retries(PortCollision::Reload)).is_true();

        let reload_pinned = LeptosTestAppConfig::new("./test-app").with_reload_port(3001);
        let retry = RetryPolicy::for_attempt(&reload_pinned, 1);
        assert_that!(retry.retries(PortCollision::App)).is_true();
        assert_that!(retry.retries(PortCollision::Reload)).is_false();
    }

    #[test]
    fn does_not_retry_on_the_final_attempt() {
        let config = LeptosTestAppConfig::new("./test-app");
        let retry = RetryPolicy::for_attempt(&config, RetryPolicy::MAX_ATTEMPTS);

        assert_that!(retry.retries(PortCollision::App)).is_false();
        assert_that!(retry.retries(PortCollision::Reload)).is_false();
    }
}

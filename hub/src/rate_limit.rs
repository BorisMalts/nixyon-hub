use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    limit: usize,
    window: Duration,
    max_keys: usize,
    hits: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

pub enum Decision {
    Allowed,
    Limited { retry_after: Duration },
}

impl RateLimiter {
    pub fn new(limit: usize, window: Duration, max_keys: usize) -> Self {
        Self {
            limit,
            window,
            max_keys,
            hits: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, key: IpAddr, now: Instant) -> Decision {
        let Ok(mut hits) = self.hits.lock() else {
            return Decision::Limited {
                retry_after: self.window,
            };
        };
        if hits.len() >= self.max_keys && !hits.contains_key(&key) {
            let window = self.window;
            hits.retain(|_, q| {
                q.back()
                    .is_some_and(|last| now.duration_since(*last) < window)
            });
            if hits.len() >= self.max_keys {
                return Decision::Limited {
                    retry_after: self.window,
                };
            }
        }
        let queue = hits.entry(key).or_default();
        while queue
            .front()
            .is_some_and(|first| now.duration_since(*first) >= self.window)
        {
            queue.pop_front();
        }
        if queue.len() >= self.limit {
            let oldest = queue.front().copied().unwrap_or(now);
            return Decision::Limited {
                retry_after: self.window.saturating_sub(now.duration_since(oldest)),
            };
        }
        queue.push_back(now);
        Decision::Allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn limit_per_key_within_window_then_recovers() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60), 100);
        let t0 = Instant::now();
        for i in 0..3 {
            assert!(matches!(
                limiter.check(ip(1), t0 + Duration::from_secs(i)),
                Decision::Allowed
            ));
        }
        match limiter.check(ip(1), t0 + Duration::from_secs(10)) {
            Decision::Limited { retry_after } => assert_eq!(retry_after, Duration::from_secs(50)),
            Decision::Allowed => panic!("должно быть ограничено"),
        }
        assert!(matches!(limiter.check(ip(2), t0), Decision::Allowed));
        assert!(matches!(
            limiter.check(ip(1), t0 + Duration::from_secs(61)),
            Decision::Allowed
        ));
    }

    #[test]
    fn table_of_keys_does_not_grow_without_bound() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60), 2);
        let t0 = Instant::now();
        assert!(matches!(limiter.check(ip(1), t0), Decision::Allowed));
        assert!(matches!(limiter.check(ip(2), t0), Decision::Allowed));
        assert!(matches!(limiter.check(ip(3), t0), Decision::Limited { .. }));
        assert!(matches!(
            limiter.check(ip(3), t0 + Duration::from_secs(61)),
            Decision::Allowed
        ));
    }
}

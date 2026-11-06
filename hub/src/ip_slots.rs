use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Сколько одновременных отдач разрешено одному адресу. Место держится, пока жив
/// выданный пропуск.
pub struct IpSlots {
    per_ip: usize,
    max_ips: usize,
    slots: Mutex<HashMap<IpAddr, Arc<Semaphore>>>,
}

impl IpSlots {
    /// `max_ips`: сколько адресов помнить; адреса без активных отдач при переполнении
    /// забываются, чтобы таблица не росла от перебора адресов.
    pub fn new(per_ip: usize, max_ips: usize) -> Self {
        Self {
            per_ip,
            max_ips,
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// None: у адреса заняты все места или таблица полна активными адресами.
    pub fn try_acquire(&self, ip: IpAddr) -> Option<OwnedSemaphorePermit> {
        let ip = client_key(ip);
        let semaphore = {
            let mut slots = self.slots.lock().ok()?;
            if !slots.contains_key(&ip) && slots.len() >= self.max_ips {
                let per_ip = self.per_ip;
                slots.retain(|_, s| s.available_permits() < per_ip);
                if slots.len() >= self.max_ips {
                    return None;
                }
            }
            Arc::clone(
                slots
                    .entry(ip)
                    .or_insert_with(|| Arc::new(Semaphore::new(self.per_ip))),
            )
        };
        semaphore.try_acquire_owned().ok()
    }
}

// Адреса IPv6 одного клиента различаются внутри его подсети /64: места считаются на
// подсеть, иначе ограничение обходится сменой адреса.
fn client_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6((u128::from(v6) & !u128::from(u64::MAX)).into()),
        },
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([203, 0, 113, last])
    }

    #[test]
    fn each_address_gets_its_own_slots_and_a_dropped_permit_frees_one() {
        let slots = IpSlots::new(2, 10);
        let a = slots.try_acquire(ip(1)).unwrap();
        let _b = slots.try_acquire(ip(1)).unwrap();
        assert!(slots.try_acquire(ip(1)).is_none());
        assert!(slots.try_acquire(ip(2)).is_some());
        drop(a);
        assert!(slots.try_acquire(ip(1)).is_some());
    }

    #[test]
    fn idle_addresses_are_forgotten_when_the_table_is_full() {
        let slots = IpSlots::new(1, 2);
        let _busy = slots.try_acquire(ip(1)).unwrap();
        drop(slots.try_acquire(ip(2)).unwrap());
        assert!(
            slots.try_acquire(ip(3)).is_some(),
            "простаивающий адрес 2 забыт"
        );
    }

    #[test]
    fn addresses_of_one_ipv6_subnet_share_the_slots() {
        let slots = IpSlots::new(1, 10);
        let v6 = |s: &str| s.parse::<IpAddr>().unwrap();
        let _a = slots.try_acquire(v6("2001:db8:1:2::1")).unwrap();
        assert!(slots.try_acquire(v6("2001:db8:1:2:ffff::9")).is_none());
        assert!(slots.try_acquire(v6("2001:db8:1:3::1")).is_some());
        let _mapped = slots.try_acquire(v6("::ffff:203.0.113.9")).unwrap();
        assert!(slots.try_acquire(ip(9)).is_none(), "тот же IPv4");
    }

    #[test]
    fn a_table_full_of_busy_addresses_refuses_a_new_one() {
        let slots = IpSlots::new(1, 2);
        let _a = slots.try_acquire(ip(1)).unwrap();
        let _b = slots.try_acquire(ip(2)).unwrap();
        assert!(slots.try_acquire(ip(3)).is_none());
    }
}

use linrdp_proto::{channel, display_control::DisplayControl};
use std::time::{Duration, Instant};
type Error = Box<dyn std::error::Error>;
/// One negotiated endpoint, one outstanding resize, and only the latest window size.
pub(super) struct Resize {
    user: u16,
    pub channel: u16,
    wire: channel::Channel,
    control: DisplayControl,
    partial: Option<Instant>,
    observed: Option<((usize, usize), Instant)>,
    last: Option<(u16, u16)>,
    pending: Option<((u16, u16), Instant)>,
    announced: bool,
}
impl Resize {
    pub fn new(user: u16, channel: u16) -> Self {
        Self {
            user,
            channel,
            wire: Default::default(),
            control: Default::default(),
            partial: None,
            observed: None,
            last: None,
            pending: None,
            announced: false,
        }
    }
    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }
    pub fn receive(&mut self, b: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
        let mut out = Vec::new();
        if let Some(message) = self.wire.receive(b)? {
            for reply in self.control.receive(&message)? {
                out.extend(channel::send_dvc(self.user, self.channel, &reply)?);
            }
        }
        if self.wire.is_partial() || self.control.partial() {
            self.partial.get_or_insert(Instant::now());
        } else {
            self.partial = None;
        }
        Ok(out)
    }
    pub fn poll(
        &mut self,
        now: Instant,
        desired: (usize, usize),
        remote: (u16, u16),
        active: bool,
        painted: bool,
    ) -> Result<Option<Vec<Vec<u8>>>, Error> {
        if self
            .partial
            .is_some_and(|t| now.duration_since(t) > Duration::from_secs(10))
        {
            return Err("display channel fragment timed out".into());
        }
        if self.observed.is_none_or(|(s, _)| s != desired) {
            self.observed = Some((desired, now));
        }
        if let Some((target, sent)) = self.pending {
            if active && painted && remote == target {
                self.pending = None;
                println!("Remote resolution changed to {}x{}.", target.0, target.1);
            } else if now.duration_since(sent) > Duration::from_secs(5) || !self.control.ready() {
                self.pending = None;
                println!("Remote resolution was not confirmed; retaining local scaling.");
            }
        }
        if !self.control.ready() {
            self.announced = false;
            return Ok(None);
        }
        if !self.announced {
            println!("Dynamic resolution ready: the remote desktop follows the window size.");
            self.announced = true;
            self.last = None;
        }
        if !active
            || !painted
            || self.waiting()
            || now.duration_since(self.observed.unwrap().1) < Duration::from_millis(300)
        {
            return Ok(None);
        }
        let Some(target) = self.control.size(desired.0, desired.1) else {
            return Ok(None);
        };
        if target == remote || self.last == Some(target) {
            return Ok(None);
        }
        let packets = channel::send_dvc(
            self.user,
            self.channel,
            &self.control.layout(target.0, target.1)?,
        )?;
        self.last = Some(target);
        self.pending = Some((target, now));
        Ok(Some(packets))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ready() -> Resize {
        let mut r = Resize::new(1004, 1005);
        r.control.receive(&[0x50, 0, 1, 0]).unwrap();
        let mut p = vec![0x10, 1];
        p.extend(b"Microsoft::Windows::RDS::DisplayControl\0");
        r.control.receive(&p).unwrap();
        let mut p = vec![0x30, 1];
        for n in [5u32, 20, 1, 4096, 4096] {
            p.extend(n.to_le_bytes());
        }
        r.control.receive(&p).unwrap();
        r
    }
    #[test]
    fn coalesces_changes_and_waits_for_confirmation() {
        let mut r = ready();
        let t = Instant::now();
        let remote = (1024, 768);
        assert!(
            r.poll(t, (1200, 800), remote, true, true)
                .unwrap()
                .is_none()
        );
        assert!(
            r.poll(
                t + Duration::from_millis(200),
                (1400, 900),
                remote,
                true,
                true
            )
            .unwrap()
            .is_none()
        );
        assert!(
            r.poll(
                t + Duration::from_millis(400),
                (1400, 900),
                remote,
                true,
                true
            )
            .unwrap()
            .is_none()
        );
        assert!(
            r.poll(
                t + Duration::from_millis(501),
                (1400, 900),
                remote,
                true,
                true
            )
            .unwrap()
            .is_some()
        );
        assert!(
            r.poll(t + Duration::from_secs(1), (1600, 1000), remote, true, true)
                .unwrap()
                .is_none()
        );
        assert!(
            r.poll(
                t + Duration::from_secs(2),
                (1600, 1000),
                (1400, 900),
                true,
                true
            )
            .unwrap()
            .is_some()
        );
        assert!(r.waiting());
    }
    #[test]
    fn timeout_restores_input_without_retry_storm() {
        let mut r = ready();
        let t = Instant::now();
        r.poll(t, (1200, 800), (1024, 768), true, true).unwrap();
        r.poll(
            t + Duration::from_secs(1),
            (1200, 800),
            (1024, 768),
            true,
            true,
        )
        .unwrap();
        assert!(r.waiting());
        assert!(
            r.poll(
                t + Duration::from_secs(7),
                (1200, 800),
                (1024, 768),
                true,
                true
            )
            .unwrap()
            .is_none()
        );
        assert!(!r.waiting());
        assert!(
            r.poll(
                t + Duration::from_secs(9),
                (1200, 800),
                (1024, 768),
                true,
                true
            )
            .unwrap()
            .is_none()
        );
        assert!(
            r.poll(
                t + Duration::from_secs(9),
                (1400, 900),
                (1024, 768),
                true,
                true
            )
            .unwrap()
            .is_none()
        );
        assert!(
            r.poll(
                t + Duration::from_millis(9300),
                (1400, 900),
                (1024, 768),
                true,
                true
            )
            .unwrap()
            .is_some()
        );
        assert!(r.waiting());
    }
    #[test]
    fn debounce_uses_the_latest_observed_size() {
        let mut r = ready();
        let t = Instant::now();
        let remote = (1024, 768);
        assert!(
            r.poll(t, (1200, 800), remote, true, true)
                .unwrap()
                .is_none()
        );
        assert!(
            r.poll(
                t + Duration::from_millis(299),
                (1400, 900),
                remote,
                true,
                true
            )
            .unwrap()
            .is_none()
        );
        assert!(
            r.poll(
                t + Duration::from_millis(598),
                (1400, 900),
                remote,
                true,
                true
            )
            .unwrap()
            .is_none()
        );
        assert!(
            r.poll(
                t + Duration::from_millis(599),
                (1400, 900),
                remote,
                true,
                true
            )
            .unwrap()
            .is_some()
        );
    }
    #[test]
    fn resize_target_is_the_full_native_window() {
        let mut r = ready();
        let t = Instant::now();
        let native_window = (1280, 720);
        assert!(
            r.poll(t, native_window, (1024, 768), true, true)
                .unwrap()
                .is_none()
        );
        let packets = r
            .poll(
                t + Duration::from_millis(300),
                native_window,
                (1024, 768),
                true,
                true,
            )
            .unwrap()
            .unwrap();
        assert_eq!(r.pending.unwrap().0, (1280, 720));
        assert_eq!(
            u32::from_le_bytes(packets[0][11..15].try_into().unwrap()),
            0x03,
            "DRDYNVC must not set CHANNEL_FLAG_SHOW_PROTOCOL"
        );
    }
}

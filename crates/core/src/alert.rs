use crate::model::{MeterKind, ProviderId};
use std::collections::HashMap;

/// Decide quando avisar: dispara uma vez por limiar cruzado e rearma no reset.
#[derive(Default)]
pub struct AlertEngine {
    fired: HashMap<(ProviderId, MeterKind), usize>, // nº de limiares já avisados
}

impl AlertEngine {
    /// Retorna o limiar recém-cruzado (o mais alto), se houver.
    pub fn check(&mut self, p: ProviderId, k: MeterKind, used_pct: f32, thresholds: &[f32]) -> Option<f32> {
        let crossed = thresholds.iter().filter(|t| used_pct >= **t).count();
        let slot = self.fired.entry((p, k)).or_insert(0);
        let prev = *slot;
        *slot = crossed; // cai se o uso caiu (reset) => rearma
        if crossed > prev { Some(thresholds[crossed - 1]) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const T: [f32; 3] = [80.0, 90.0, 95.0];

    #[test]
    fn fires_once_per_threshold_and_rearms() {
        let mut e = AlertEngine::default();
        let (p, k) = (ProviderId::Claude, MeterKind::FiveHour);
        assert_eq!(e.check(p, k, 50.0, &T), None);
        assert_eq!(e.check(p, k, 82.0, &T), Some(80.0));
        assert_eq!(e.check(p, k, 85.0, &T), None);
        assert_eq!(e.check(p, k, 96.0, &T), Some(95.0));
        assert_eq!(e.check(p, k, 5.0, &T), None); // reset
        assert_eq!(e.check(p, k, 81.0, &T), Some(80.0));
    }
}

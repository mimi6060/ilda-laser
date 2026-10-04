//! Evaluation metrics (T-244), rewritten after the definitions mir_eval
//! documents (MIT; no code copied): one-to-one event matching within a
//! tolerance window and its F-measure, tempo accuracy Acc1 (±4 %) and
//! Acc2 (±4 % up to an octave or a factor 3).

/// Pairs `(truth index, found index)` matched one-to-one within ±`tol` s,
/// each truth event taking the closest unused detection. Both sorted.
pub fn match_events(truth: &[f64], found: &[f64], tol: f64) -> Vec<(usize, usize)> {
    let mut used = vec![false; found.len()];
    let mut pairs = Vec::new();
    let mut lo = 0;
    for (i, &t) in truth.iter().enumerate() {
        while lo < found.len() && found[lo] < t - tol {
            lo += 1;
        }
        let best = (lo..found.len()).take_while(|&j| found[j] <= t + tol).filter(|&j| !used[j]).min_by(|&a, &b| (found[a] - t).abs().total_cmp(&(found[b] - t).abs()));
        if let Some(j) = best {
            used[j] = true;
            pairs.push((i, j));
        }
    }
    pairs
}

/// F-measure of `found` against `truth` within ±`tol` s (1 when both are
/// empty, as mir_eval).
pub fn f_measure(truth: &[f64], found: &[f64], tol: f64) -> f32 {
    if truth.is_empty() && found.is_empty() {
        return 1.0;
    }
    2.0 * match_events(truth, found, tol).len() as f32 / (truth.len() + found.len()) as f32
}

/// Report delays (s) of the detections after each true event: the first
/// unused report in `t − early ..= t + late`.
pub fn delays(truth: &[f64], reports: &[f64], early: f64, late: f64) -> Vec<f64> {
    let mut used = vec![false; reports.len()];
    let mut out = Vec::new();
    for &t in truth {
        if let Some(j) = (0..reports.len()).find(|&j| !used[j] && reports[j] >= t - early && reports[j] <= t + late) {
            used[j] = true;
            out.push(reports[j] - t);
        }
    }
    out
}

/// Tempo within 4 % of the truth.
pub fn acc1(est: f64, truth: f64) -> bool {
    truth > 0.0 && (est / truth - 1.0).abs() <= 0.04
}

/// Tempo within 4 % of the truth or of ×2, ×½, ×3, ×⅓ of it.
pub fn acc2(est: f64, truth: f64) -> bool {
    [1.0, 2.0, 0.5, 3.0, 1.0 / 3.0].iter().any(|k| acc1(est, truth * k))
}

/// Median (`p = 0.5`) or another quantile, nearest rank; `None` if empty.
pub fn quantile(v: &[f64], p: f64) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let k = ((p * s.len() as f64).ceil() as usize).clamp(1, s.len()) - 1;
    Some(s[k])
}

/// Mean of the values present; `None` if none is.
pub fn mean(v: impl IntoIterator<Item = Option<f64>>) -> Option<f64> {
    let (mut sum, mut n) = (0.0, 0);
    for x in v.into_iter().flatten() {
        sum += x;
        n += 1;
    }
    (n > 0).then(|| sum / n as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_match_one_to_one_within_the_window() {
        let truth = [1.0, 2.0, 3.0, 4.0];
        // 2.04 is in, 3.06 is out, 1.01 beats 0.98 to 1.0.
        let found = [0.98, 1.01, 2.04, 3.06, 5.0];
        let pairs = match_events(&truth, &found, 0.05);
        assert_eq!(pairs, [(0, 1), (1, 2)]);
        assert!((f_measure(&truth, &found, 0.05) - 4.0 / 9.0).abs() < 1e-6);
        assert_eq!(f_measure(&truth, &truth, 0.05), 1.0);
        assert_eq!(f_measure(&[], &[], 0.05), 1.0);
        assert_eq!(f_measure(&truth, &[], 0.05), 0.0);
        // A detection is never used twice.
        assert_eq!(match_events(&[1.0, 1.02], &[1.01], 0.05).len(), 1);
    }

    #[test]
    fn delays_take_the_first_report_after_each_event() {
        let d = delays(&[1.0, 2.0, 3.0], &[1.012, 1.02, 2.2, 3.004], 0.005, 0.1);
        assert_eq!(d.len(), 2);
        assert!((d[0] - 0.012).abs() < 1e-9 && (d[1] - 0.004).abs() < 1e-9);
    }

    #[test]
    fn tempo_accuracies() {
        assert!(acc1(128.0, 125.0) && !acc1(120.0, 128.0));
        assert!(!acc1(64.0, 128.0) && acc2(64.0, 128.0) && acc2(256.0, 128.0) && acc2(43.0, 128.0));
        assert!(!acc2(100.0, 128.0) && !acc2(128.0, 0.0));
    }

    #[test]
    fn quantiles_and_means() {
        assert_eq!(quantile(&[3.0, 1.0, 2.0], 0.5), Some(2.0));
        assert_eq!(quantile(&[3.0, 1.0, 2.0], 1.0), Some(3.0));
        assert_eq!(quantile(&[], 0.5), None);
        assert_eq!(mean([Some(1.0), None, Some(3.0)]), Some(2.0));
        assert_eq!(mean([None]), None);
    }
}

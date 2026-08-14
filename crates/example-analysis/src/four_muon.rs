//! The four-muon selection, a direct port of `fourMuonMass.C`.
//!
//! It doubles as the worked example of an analysis written against
//! [`nanoaod`]: a collection declared with [`nano_object!`](nanoaod::nano_object), a per-event
//! selection over [`combinations`], and a row type handed to a
//! [`TreeWriter`](root_io::write::tree::TreeWriter).
//!
//! Arithmetic follows the original exactly: muon kinematics are read as
//! `Float_t`, promoted to `double` for the Lorentz algebra (as C++ does when
//! passing them to `TLorentzVector::SetPtEtaPhiM`), and the Z candidate masses
//! are stored back into `float` before the window cuts and the scoring — the
//! rounding this introduces is part of the selection and must be reproduced.

use nanoaod::prelude::*;

nano_object! {
    /// A reconstructed muon, as NanoAOD stores it.
    pub struct Muon in "Muon" {
        pt: f32,
        eta: f32,
        phi: f32,
        mass: f32,
        charge: i32,
        /// The medium working point of the muon identification.
        medium_id: bool = "mediumId",
        /// Particle-flow relative isolation in a cone of 0.4.
        iso: f32 = "pfRelIso04_all",
    }
}

impl_four_momentum!(Muon);

impl Muon {
    /// The per-muon quality cuts: loose enough to keep the four legs of a
    /// `H → ZZ* → 4μ` decay, tight enough to drop hadronic fakes.
    pub fn passes_selection(&self) -> bool {
        f64::from(self.pt) > 5.0
            && f64::from(self.eta).abs() < 2.4
            && self.medium_id
            && f64::from(self.iso) < 0.4
    }
}

/// The four-muon candidate an event yields, and the row written for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub run: u32,
    pub luminosity_block: u32,
    pub event: u64,
    pub four_muon_mass: f32,
    pub z1_mass: f32,
    pub z2_mass: f32,
    /// The four muons, ordered by descending transverse momentum.
    pub muons: [Muon; 4],
}

impl Candidate {
    pub fn pt(&self) -> [f32; 4] {
        self.muons.map(|m| m.pt)
    }

    pub fn eta(&self) -> [f32; 4] {
        self.muons.map(|m| m.eta)
    }

    pub fn phi(&self) -> [f32; 4] {
        self.muons.map(|m| m.phi)
    }

    pub fn charge(&self) -> [i32; 4] {
        self.muons.map(|m| m.charge)
    }
}

/// A quadruplet that passed every cut, with the pairing that scored best.
///
/// Keeping the winner in an `Option<Winner>` rather than in a set of parallel
/// `best_*` variables is what makes "no candidate" unrepresentable as a stale
/// score.
#[derive(Debug, Clone, Copy)]
struct Winner {
    muons: [Muon; 4],
    z1_mass: f32,
    z2_mass: f32,
    /// Distance of the leading Z from the nominal mass; lower wins.
    score: f64,
}

/// The three ways to split four muons into two pairs.
const PAIRINGS: [[usize; 4]; 3] = [[0, 1, 2, 3], [0, 2, 1, 3], [0, 3, 1, 2]];

/// Applies the per-event selection, returning the best candidate if any.
///
/// ```
/// use example_analysis::four_muon::{select_event, Muon};
///
/// // Fewer than four muons can never yield a candidate.
/// assert!(select_event(&[]).is_none());
/// ```
pub fn select_event(muons: &[Muon]) -> Option<Candidate> {
    let good: Vec<Muon> = muons
        .iter()
        .copied()
        .filter(Muon::passes_selection)
        .collect();
    if good.len() < 4 {
        return None;
    }

    let best = combinations::<_, 4>(&good)
        .filter_map(|quad| score_quadruplet(quad.map(|m| *m)))
        .min_by(|a, b| a.score.total_cmp(&b.score))?;

    Some(Candidate {
        run: 0,
        luminosity_block: 0,
        event: 0,
        four_muon_mass: invariant_mass(best.muons.iter()) as f32,
        z1_mass: best.z1_mass,
        z2_mass: best.z2_mass,
        muons: best.muons,
    })
}

/// Applies the quadruplet-level cuts and picks the best pairing, or `None` if
/// the quadruplet fails.
fn score_quadruplet(mut quad: [Muon; 4]) -> Option<Winner> {
    // Order by descending pt. `sort_by` is stable, which matches the insertion
    // sort libstdc++ uses for four elements.
    quad.sort_by(|a, b| b.pt.total_cmp(&a.pt));

    // The two leading muons must be hard enough to have fired the trigger.
    if f64::from(quad[0].pt) <= 20.0 || f64::from(quad[1].pt) <= 10.0 {
        return None;
    }
    if quad.iter().map(|m| m.charge).sum::<i32>() != 0 {
        return None;
    }

    // Every opposite-sign pair must sit above the low-mass resonances.
    let above_resonances = combinations::<_, 2>(&quad)
        .all(|[a, b]| a.charge == b.charge || invariant_mass([a, b]) > 4.0);
    if !above_resonances {
        return None;
    }

    let p4 = quad.map(|m| m.p4());

    PAIRINGS
        .iter()
        .filter(|&&[i, j, k, l]| {
            quad[i].charge != quad[j].charge && quad[k].charge != quad[l].charge
        })
        .filter_map(|&[i, j, k, l]| {
            // Stored as float, exactly as the original does.
            let mut z1 = (p4[i] + p4[j]).mass() as f32;
            let mut z2 = (p4[k] + p4[l]).mass() as f32;
            if (f64::from(z2) - Z_MASS).abs() < (f64::from(z1) - Z_MASS).abs() {
                std::mem::swap(&mut z1, &mut z2);
            }
            let in_window =
                (40.0..=120.0).contains(&f64::from(z1)) && (12.0..=120.0).contains(&f64::from(z2));
            in_window.then(|| Winner {
                muons: quad,
                z1_mass: z1,
                z2_mass: z2,
                score: (f64::from(z1) - Z_MASS).abs(),
            })
        })
        .min_by(|a, b| a.score.total_cmp(&b.score))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A muon that passes the quality cuts, so tests vary only what they mean
    /// to vary.
    fn muon(pt: f32, eta: f32, phi: f32, charge: i32) -> Muon {
        Muon {
            pt,
            eta,
            phi,
            mass: MUON_MASS as f32,
            charge,
            medium_id: true,
            iso: 0.0,
        }
    }

    /// Four muons that form a candidate: two opposite-sign pairs, back to back
    /// and hard enough for the leading-pt cuts.
    fn good_quadruplet() -> Vec<Muon> {
        vec![
            muon(60.0, 0.4, 0.0, 1),
            muon(50.0, -0.4, std::f64::consts::PI as f32, -1),
            muon(40.0, 0.2, 1.0, 1),
            muon(30.0, -0.2, 1.0 + std::f64::consts::PI as f32, -1),
        ]
    }

    #[test]
    fn fewer_than_four_good_muons_yields_nothing() {
        assert!(select_event(&[]).is_none());
        let three = &good_quadruplet()[..3];
        assert!(select_event(three).is_none());
    }

    #[test]
    fn a_clean_quadruplet_is_selected() {
        let c = select_event(&good_quadruplet()).expect("should select");
        assert!(c.four_muon_mass > 0.0);
        // Muons come back ordered by descending pt.
        assert_eq!(c.pt(), [60.0, 50.0, 40.0, 30.0]);
        assert_eq!(c.charge().iter().sum::<i32>(), 0);
        // The leading Z is the one nearer the nominal mass.
        assert!((f64::from(c.z1_mass) - Z_MASS).abs() <= (f64::from(c.z2_mass) - Z_MASS).abs());
    }

    #[test]
    fn quality_cuts_reject_muons_the_analysis_should_not_see() {
        assert!(muon(30.0, 0.0, 0.0, 1).passes_selection());
        // Too soft, too forward, failing the id, and not isolated.
        assert!(!muon(4.0, 0.0, 0.0, 1).passes_selection());
        assert!(!muon(30.0, 3.0, 0.0, 1).passes_selection());
        assert!(!Muon {
            medium_id: false,
            ..muon(30.0, 0.0, 0.0, 1)
        }
        .passes_selection());
        assert!(!Muon {
            iso: 0.5,
            ..muon(30.0, 0.0, 0.0, 1)
        }
        .passes_selection());
    }

    #[test]
    fn a_non_neutral_quadruplet_is_rejected() {
        let mut muons = good_quadruplet();
        muons[3].charge = 1;
        assert!(select_event(&muons).is_none());
    }

    #[test]
    fn soft_leading_muons_are_rejected() {
        // Scaled down so the two leading muons miss the 20/10 GeV thresholds.
        let muons: Vec<Muon> = good_quadruplet()
            .into_iter()
            .map(|m| Muon { pt: 8.0, ..m })
            .collect();
        assert!(select_event(&muons).is_none());
    }

    #[test]
    fn extra_muons_do_not_change_a_clean_candidate() {
        let base = select_event(&good_quadruplet()).expect("should select");
        let mut muons = good_quadruplet();
        // A fifth muon that fails the quality cuts is invisible to the result.
        muons.push(Muon {
            iso: 0.9,
            ..muon(45.0, 0.1, 2.0, -1)
        });
        let with_extra = select_event(&muons).expect("should still select");
        assert_eq!(base, with_extra);
    }
}

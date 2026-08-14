//! Relativistic kinematics.
//!
//! Four-vectors, the [`FourMomentum`] trait every physics object implements,
//! and the combinatorics selections are built from. Nothing here knows about
//! files or experiments, so it is usable on its own.
//!
//! ```
//! use lorentzvector::{invariant_mass, FourMomentum, LorentzVector};
//!
//! let a = LorentzVector::from_pt_eta_phi_m(30.0, 0.5, 0.0, 0.105);
//! let b = LorentzVector::from_pt_eta_phi_m(25.0, -0.5, std::f64::consts::PI, 0.105);
//! let m = invariant_mass([&a, &b]);
//! assert!(m > 0.0);
//! ```

use std::ops::{Add, AddAssign, Sub};

/// Nominal Z boson mass in GeV, the PDG value CMS analyses rank pairings by.
pub const Z_MASS: f64 = 91.1876;
/// Nominal muon mass in GeV.
pub const MUON_MASS: f64 = 0.105_658_374_5;

/// A Lorentz vector in the same `(x, y, z, t)` representation as ROOT's
/// `TLorentzVector`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LorentzVector {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub t: f64,
}

impl LorentzVector {
    /// `TLorentzVector::SetPtEtaPhiM`.
    pub fn from_pt_eta_phi_m(pt: f64, eta: f64, phi: f64, m: f64) -> Self {
        let pt = pt.abs();
        let x = pt * phi.cos();
        let y = pt * phi.sin();
        let z = pt * eta.sinh();
        // SetXYZM: a negative mass is treated as spacelike.
        let t = if m >= 0.0 {
            (x * x + y * y + z * z + m * m).sqrt()
        } else {
            (x * x + y * y + z * z - m * m).max(0.0).sqrt()
        };
        Self { x, y, z, t }
    }

    /// `TLorentzVector::M()`, which keeps the sign of `E² - p²`.
    pub fn mass(&self) -> f64 {
        let mag2 = self.t * self.t - self.momentum_squared();
        if mag2 < 0.0 {
            -(-mag2).sqrt()
        } else {
            mag2.sqrt()
        }
    }

    /// Squared momentum, `|p|²`.
    pub fn momentum_squared(&self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    /// Transverse momentum.
    pub fn pt(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    /// Energy, the time component.
    pub fn energy(&self) -> f64 {
        self.t
    }

    /// Pseudorapidity. Infinite for a vector exactly along the beam.
    pub fn eta(&self) -> f64 {
        let p = self.momentum_squared().sqrt();
        if p == self.z.abs() {
            return f64::INFINITY * self.z.signum();
        }
        0.5 * ((p + self.z) / (p - self.z)).ln()
    }

    /// Azimuth in `(-π, π]`.
    pub fn phi(&self) -> f64 {
        self.y.atan2(self.x)
    }
}

impl Add for LorentzVector {
    type Output = Self;

    fn add(self, o: Self) -> Self {
        Self {
            x: self.x + o.x,
            y: self.y + o.y,
            z: self.z + o.z,
            t: self.t + o.t,
        }
    }
}

impl Sub for LorentzVector {
    type Output = Self;

    fn sub(self, o: Self) -> Self {
        Self {
            x: self.x - o.x,
            y: self.y - o.y,
            z: self.z - o.z,
            t: self.t - o.t,
        }
    }
}

impl AddAssign for LorentzVector {
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

impl std::iter::Sum for LorentzVector {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), Add::add)
    }
}

/// Anything with a four-momentum: a muon, a jet, a reconstructed candidate.
///
/// Implement it for a physics object and the derived quantities come for free.
/// [`impl_four_momentum!`] writes the implementation for any struct with
/// `pt`, `eta`, `phi` and `mass` fields.
pub trait FourMomentum {
    fn pt(&self) -> f64;
    fn eta(&self) -> f64;
    fn phi(&self) -> f64;
    fn mass(&self) -> f64;

    /// The object as a Lorentz vector.
    fn p4(&self) -> LorentzVector {
        LorentzVector::from_pt_eta_phi_m(self.pt(), self.eta(), self.phi(), self.mass())
    }

    /// Signed azimuthal separation, wrapped into `(-π, π]`.
    fn delta_phi(&self, other: &impl FourMomentum) -> f64 {
        delta_phi(self.phi(), other.phi())
    }

    /// Separation in the `(η, φ)` plane, the cone variable used for isolation
    /// and matching.
    fn delta_r(&self, other: &impl FourMomentum) -> f64 {
        let deta = self.eta() - other.eta();
        let dphi = self.delta_phi(other);
        (deta * deta + dphi * dphi).sqrt()
    }
}

impl FourMomentum for LorentzVector {
    fn pt(&self) -> f64 {
        LorentzVector::pt(self)
    }
    fn eta(&self) -> f64 {
        LorentzVector::eta(self)
    }
    fn phi(&self) -> f64 {
        LorentzVector::phi(self)
    }
    fn mass(&self) -> f64 {
        LorentzVector::mass(self)
    }
    fn p4(&self) -> LorentzVector {
        *self
    }
}

/// Implements [`FourMomentum`] for a type whose `pt`, `eta`, `phi` and `mass`
/// fields are any numeric type.
///
/// ```
/// use lorentzvector::{impl_four_momentum, FourMomentum};
///
/// struct Jet { pt: f32, eta: f32, phi: f32, mass: f32 }
/// impl_four_momentum!(Jet);
///
/// let j = Jet { pt: 40.0, eta: 1.0, phi: 0.0, mass: 8.0 };
/// assert!((j.p4().pt() - 40.0).abs() < 1e-5);
/// ```
#[macro_export]
macro_rules! impl_four_momentum {
    ($t:ty) => {
        impl $crate::FourMomentum for $t {
            fn pt(&self) -> f64 {
                f64::from(self.pt)
            }
            fn eta(&self) -> f64 {
                f64::from(self.eta)
            }
            fn phi(&self) -> f64 {
                f64::from(self.phi)
            }
            fn mass(&self) -> f64 {
                f64::from(self.mass)
            }
        }
    };
}

/// Signed difference of two azimuths, wrapped into `(-π, π]`.
pub fn delta_phi(a: f64, b: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    let mut d = (a - b) % TAU;
    if d > PI {
        d -= TAU;
    } else if d <= -PI {
        d += TAU;
    }
    d
}

/// Invariant mass of a group of objects.
///
/// ```
/// use lorentzvector::{invariant_mass, LorentzVector};
/// let a = LorentzVector::from_pt_eta_phi_m(10.0, 0.0, 0.0, 0.0);
/// let b = LorentzVector::from_pt_eta_phi_m(10.0, 0.0, std::f64::consts::PI, 0.0);
/// assert!((invariant_mass([&a, &b]) - 20.0).abs() < 1e-9);
/// ```
pub fn invariant_mass<'a, P, I>(objects: I) -> f64
where
    P: FourMomentum + 'a,
    I: IntoIterator<Item = &'a P>,
{
    objects
        .into_iter()
        .map(FourMomentum::p4)
        .sum::<LorentzVector>()
        .mass()
}

/// Invariant mass of a group of four-vectors.
///
/// [`invariant_mass`] takes objects that are all the same type. Use this one
/// to combine different kinds — a muon and a photon, a lepton and a jet — by
/// converting each with [`FourMomentum::p4`] first, which is what makes them
/// addable at all.
///
/// ```
/// use lorentzvector::{mass_of, FourMomentum, LorentzVector};
///
/// struct Muon { pt: f32, eta: f32, phi: f32, mass: f32 }
/// lorentzvector::impl_four_momentum!(Muon);
///
/// let mu = Muon { pt: 40.0, eta: 0.3, phi: 0.0, mass: 0.106 };
/// let photon = LorentzVector::from_pt_eta_phi_m(5.0, 0.3, 0.1, 0.0);
///
/// let with_photon = mass_of([mu.p4(), photon]);
/// assert!(with_photon > mu.p4().mass());
/// ```
pub fn mass_of(vectors: impl IntoIterator<Item = LorentzVector>) -> f64 {
    vectors.into_iter().sum::<LorentzVector>().mass()
}

/// Total charge of a group of objects.
pub fn total_charge<'a, T, I>(objects: I, charge: impl Fn(&T) -> i32) -> i32
where
    T: 'a,
    I: IntoIterator<Item = &'a T>,
{
    objects.into_iter().map(charge).sum()
}

/// Every `K`-element combination of `items`, in the order the indices ascend.
///
/// ```
/// use lorentzvector::combinations;
/// let xs = [1, 2, 3, 4];
/// let pairs: Vec<_> = combinations::<_, 2>(&xs).map(|[a, b]| (*a, *b)).collect();
/// assert_eq!(pairs, [(1, 2), (1, 3), (1, 4), (2, 3), (2, 4), (3, 4)]);
/// ```
pub fn combinations<T, const K: usize>(items: &[T]) -> Combinations<'_, T, K> {
    Combinations {
        items,
        indices: std::array::from_fn(|i| i),
        done: items.len() < K,
    }
}

/// Iterator over the `K`-element combinations of a slice; see
/// [`combinations`].
pub struct Combinations<'a, T, const K: usize> {
    items: &'a [T],
    indices: [usize; K],
    done: bool,
}

impl<'a, T, const K: usize> Iterator for Combinations<'a, T, K> {
    type Item = [&'a T; K];

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let current = self.indices.map(|i| &self.items[i]);

        // Advance the rightmost index that still has room, then repack the
        // ones after it — the standard lexicographic successor.
        match (0..K).rev().find(|&i| self.indices[i] < self.items.len() - K + i) {
            Some(i) => {
                self.indices[i] += 1;
                for j in i + 1..K {
                    self.indices[j] = self.indices[j - 1] + 1;
                }
            }
            None => self.done = true,
        }
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn massless_back_to_back_pair_has_twice_the_energy() {
        let a = LorentzVector::from_pt_eta_phi_m(10.0, 0.0, 0.0, 0.0);
        let b = LorentzVector::from_pt_eta_phi_m(10.0, 0.0, std::f64::consts::PI, 0.0);
        assert!(((a + b).mass() - 20.0).abs() < 1e-9);
    }

    #[test]
    fn single_particle_mass_is_recovered() {
        let p = LorentzVector::from_pt_eta_phi_m(35.0, 1.3, 0.7, MUON_MASS);
        assert!((p.mass() - MUON_MASS).abs() < 1e-9);
    }

    #[test]
    fn spacelike_vectors_get_a_negative_mass() {
        let p = LorentzVector {
            x: 5.0,
            y: 0.0,
            z: 0.0,
            t: 3.0,
        };
        assert!((p.mass() + 4.0).abs() < 1e-12);
    }

    #[test]
    fn kinematics_round_trip_through_the_vector() {
        let p = LorentzVector::from_pt_eta_phi_m(42.0, -1.7, 2.1, 5.0);
        assert!((p.pt() - 42.0).abs() < 1e-9);
        assert!((p.eta() + 1.7).abs() < 1e-9);
        assert!((p.phi() - 2.1).abs() < 1e-9);
        assert!((p.mass() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn delta_phi_wraps_across_the_discontinuity() {
        use std::f64::consts::PI;
        assert!((delta_phi(3.0, -3.0) - (6.0 - 2.0 * PI)).abs() < 1e-12);
        assert!(delta_phi(0.1, -0.1).abs() < 0.3);
        assert!(delta_phi(PI, -PI).abs() < 1e-12);
    }

    #[test]
    fn delta_r_measures_the_cone() {
        let a = LorentzVector::from_pt_eta_phi_m(10.0, 0.0, 0.0, 0.0);
        let b = LorentzVector::from_pt_eta_phi_m(10.0, 0.3, 0.4, 0.0);
        assert!((a.delta_r(&b) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn combinations_cover_every_group_once() {
        let xs = [1, 2, 3, 4, 5];
        assert_eq!(combinations::<_, 1>(&xs).count(), 5);
        assert_eq!(combinations::<_, 2>(&xs).count(), 10);
        assert_eq!(combinations::<_, 3>(&xs).count(), 10);
        assert_eq!(combinations::<_, 4>(&xs).count(), 5);
        assert_eq!(combinations::<_, 5>(&xs).count(), 1);

        let all: Vec<[i32; 4]> = combinations::<_, 4>(&xs)
            .map(|c| c.map(|x| *x))
            .collect();
        assert_eq!(all[0], [1, 2, 3, 4]);
        assert_eq!(all[4], [2, 3, 4, 5]);
    }

    #[test]
    fn combinations_of_too_short_a_slice_are_empty() {
        let xs = [1, 2];
        assert_eq!(combinations::<_, 4>(&xs).count(), 0);
        let empty: [i32; 0] = [];
        assert_eq!(combinations::<_, 1>(&empty).count(), 0);
    }

    #[test]
    fn invariant_mass_sums_the_vectors() {
        let ps: Vec<LorentzVector> = (0..4)
            .map(|i| LorentzVector::from_pt_eta_phi_m(20.0, 0.0, i as f64, MUON_MASS))
            .collect();
        let by_hand: LorentzVector = ps.iter().copied().sum();
        assert_eq!(invariant_mass(&ps), by_hand.mass());
    }
}

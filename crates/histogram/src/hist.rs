//! The histogram model and its ROOT record format.

use crate::error::{Error, Result};
use root_io::buffer::RBuffer;
use root_io::wbuffer::WBuffer;

/// Class versions ROOT 6 writes; also what [`Histogram::parse`] expects.
const TH1_VERSION: u16 = 8;
const TH2_VERSION: u16 = 5;
/// `kMustCleanup`, set on histograms attached to a directory.
const K_MUST_CLEANUP: u32 = 0x0000_0008;
/// ROOT's "not set" sentinel for `fMaximum` and `fMinimum`.
const UNSET_LIMIT: f64 = -1111.0;

/// The per-bin storage type: what distinguishes `TH1C` from `TH1D`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    /// `Char_t`, ROOT's `TH1C`/`TH2C`.
    I8,
    /// `Short_t`, ROOT's `TH1S`/`TH2S`.
    I16,
    /// `Int_t`, ROOT's `TH1I`/`TH2I`.
    I32,
    /// `Long64_t`, ROOT's `TH1L`/`TH2L`.
    I64,
    /// `Float_t`, ROOT's `TH1F`/`TH2F`.
    F32,
    /// `Double_t`, ROOT's `TH1D`/`TH2D`.
    F64,
}

impl Precision {
    /// The letter ROOT appends to the class name.
    pub fn suffix(self) -> char {
        match self {
            Precision::I8 => 'C',
            Precision::I16 => 'S',
            Precision::I32 => 'I',
            Precision::I64 => 'L',
            Precision::F32 => 'F',
            Precision::F64 => 'D',
        }
    }

    pub fn from_suffix(c: char) -> Option<Self> {
        Some(match c {
            'C' => Precision::I8,
            'S' => Precision::I16,
            'I' => Precision::I32,
            'L' => Precision::I64,
            'F' => Precision::F32,
            'D' => Precision::F64,
            _ => return None,
        })
    }

    /// Largest magnitude the integer flavours store; `None` for the floats.
    ///
    /// ROOT saturates one short of the type's minimum, symmetrically about
    /// zero, so `TH1C` holds -127..=127 rather than -128..=127.
    fn saturation(self) -> Option<i64> {
        Some(match self {
            Precision::I8 => 127,
            Precision::I16 => 32_767,
            Precision::I32 => 2_147_483_647,
            Precision::I64 => i64::MAX,
            Precision::F32 | Precision::F64 => return None,
        })
    }

    /// A value as this precision stores it (`TH1::UpdateBinContent`).
    fn store(self, v: f64) -> f64 {
        match self {
            Precision::F64 => v,
            Precision::F32 => v as f32 as f64,
            _ => {
                let limit = self.saturation().unwrap_or(i64::MAX);
                (v.trunc() as i64).clamp(-limit, limit) as f64
            }
        }
    }

    /// `content + w` as this precision accumulates it (`TH1::AddBinContent`).
    ///
    /// A `TH1F` adds in `float`, so the weight is rounded before the addition
    /// and again after it; the integer flavours truncate the weight towards
    /// zero and saturate instead of wrapping.
    fn add(self, v: f64, w: f64) -> f64 {
        match self {
            Precision::F64 => v + w,
            Precision::F32 => (v as f32 + w as f32) as f64,
            _ => {
                let limit = self.saturation().unwrap_or(i64::MAX);
                let sum = (v as i64).saturating_add(w.trunc() as i64);
                sum.clamp(-limit, limit) as f64
            }
        }
    }

    /// Writes a `TArray*` member: the element count, then the elements.
    fn write_array(self, w: &mut WBuffer, v: &[f64]) {
        w.i32(v.len() as i32);
        for &x in v {
            match self {
                Precision::I8 => w.u8(x as i8 as u8),
                Precision::I16 => w.i16(x as i16),
                Precision::I32 => w.i32(x as i32),
                Precision::I64 => w.i64(x as i64),
                Precision::F32 => w.f32(x as f32),
                Precision::F64 => w.f64(x),
            }
        }
    }

    /// Bytes one element occupies on disk.
    fn width(self) -> usize {
        match self {
            Precision::I8 => 1,
            Precision::I16 => 2,
            Precision::I32 | Precision::F32 => 4,
            Precision::I64 | Precision::F64 => 8,
        }
    }

    fn read_array(self, r: &mut RBuffer, n: usize) -> Result<Vec<f64>> {
        // Checked before allocating, so a corrupt count cannot ask for a
        // vector the record could not possibly hold.
        if n * self.width() > r.remaining() {
            return Err(Error::decode(format!(
                "an array of {n} elements does not fit in the {} bytes left",
                r.remaining()
            )));
        }
        (0..n)
            .map(|_| {
                Ok(match self {
                    Precision::I8 => r.i8()? as f64,
                    Precision::I16 => r.i16()? as f64,
                    Precision::I32 => r.i32()? as f64,
                    Precision::I64 => r.i64()? as f64,
                    Precision::F32 => r.f32()? as f64,
                    Precision::F64 => r.f64()?,
                })
            })
            .collect()
    }
}

/// One axis: `nbins` bins spanning `min` to `max`, uniform or with explicit
/// edges.
///
/// Bin 0 is the underflow and bin `nbins + 1` the overflow, as in ROOT.
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    /// The axis title, which is what ROOT draws alongside it.
    pub title: String,
    nbins: usize,
    min: f64,
    max: f64,
    /// `nbins + 1` edges, or empty for a uniform axis — ROOT's `fXbins`.
    edges: Vec<f64>,
}

impl Axis {
    /// `nbins` equal-width bins between `min` and `max`.
    pub fn uniform(nbins: usize, min: f64, max: f64) -> Result<Self> {
        if nbins == 0 {
            return Err(Error::axis("an axis needs at least one bin"));
        }
        if !min.is_finite() || !max.is_finite() || min >= max {
            return Err(Error::axis(format!(
                "axis range [{min}, {max}] is not a finite increasing interval"
            )));
        }
        Ok(Self {
            title: String::new(),
            nbins,
            min,
            max,
            edges: Vec::new(),
        })
    }

    /// An axis with explicit bin edges: `n + 1` increasing values give `n`
    /// bins.
    pub fn variable(edges: &[f64]) -> Result<Self> {
        if edges.len() < 2 {
            return Err(Error::axis("a variable-bin axis needs at least two edges"));
        }
        if !edges.iter().all(|e| e.is_finite()) {
            return Err(Error::axis("axis edges must all be finite"));
        }
        if edges.windows(2).any(|w| w[0] >= w[1]) {
            return Err(Error::axis("axis edges must be strictly increasing"));
        }
        Ok(Self {
            title: String::new(),
            nbins: edges.len() - 1,
            min: edges[0],
            max: edges[edges.len() - 1],
            edges: edges.to_vec(),
        })
    }

    /// The degenerate one-bin axis ROOT gives the dimensions a histogram does
    /// not use; it still carries that axis's title.
    fn degenerate() -> Self {
        Self {
            title: String::new(),
            nbins: 1,
            min: 0.0,
            max: 1.0,
            edges: Vec::new(),
        }
    }

    pub fn titled(mut self, title: &str) -> Self {
        self.title = title.to_string();
        self
    }

    pub fn nbins(&self) -> usize {
        self.nbins
    }

    pub fn min(&self) -> f64 {
        self.min
    }

    pub fn max(&self) -> f64 {
        self.max
    }

    /// Whether the bins are equal-width, which is how ROOT stores the axis
    /// (an empty `fXbins`).
    pub fn is_uniform(&self) -> bool {
        self.edges.is_empty()
    }

    /// Low edge of `bin`, counting from 1; `edge(nbins + 1)` is [`Axis::max`].
    ///
    /// Out-of-range bins extrapolate the way `TAxis::GetBinLowEdge` does for a
    /// uniform axis, and clamp for a variable one.
    pub fn edge(&self, bin: usize) -> f64 {
        if self.edges.is_empty() {
            let width = (self.max - self.min) / self.nbins as f64;
            self.min + (bin as f64 - 1.0) * width
        } else {
            let i = (bin.max(1) - 1).min(self.nbins);
            self.edges[i]
        }
    }

    /// The `nbins + 1` bin edges, computed for a uniform axis.
    pub fn edges(&self) -> Vec<f64> {
        if self.edges.is_empty() {
            (1..=self.nbins + 1).map(|b| self.edge(b)).collect()
        } else {
            self.edges.clone()
        }
    }

    pub fn bin_center(&self, bin: usize) -> f64 {
        0.5 * (self.edge(bin) + self.edge(bin + 1))
    }

    pub fn bin_width(&self, bin: usize) -> f64 {
        self.edge(bin + 1) - self.edge(bin)
    }

    /// `TAxis::FindBin`: 0 is the underflow, `nbins + 1` the overflow.
    pub fn find_bin(&self, x: f64) -> usize {
        if x < self.min {
            0
        } else if !matches!(x.partial_cmp(&self.max), Some(std::cmp::Ordering::Less)) {
            // Anything not below the top edge overflows — including NaN, which
            // compares to nothing and lands here as it does in ROOT.
            self.nbins + 1
        } else if self.edges.is_empty() {
            1 + (self.nbins as f64 * (x - self.min) / (self.max - self.min)) as usize
        } else {
            // The last edge not above x; `min <= x < max` keeps this in range.
            self.edges.partition_point(|&e| e <= x)
        }
    }
}

/// A one- or two-dimensional histogram.
///
/// Built with [`Histogram::h1`] or [`Histogram::h2`], filled with
/// [`Histogram::fill`] or [`Histogram::fill_xy`], and moved to and from a ROOT
/// file with [`Histogram::serialize`] and [`Histogram::parse`].
///
/// Bins are numbered as in ROOT: for a one-dimensional histogram bin 0 is the
/// underflow and bin `nbins + 1` the overflow; for a two-dimensional one the
/// global bin is `iy * (nbins_x + 2) + ix`, which [`Histogram::global_bin`]
/// computes.
///
/// Everything ROOT needs to draw the histogram survives a round trip except
/// the display-only members this crate does not model — the contour levels,
/// the draw option, the fill buffer and the attached function list, which are
/// read as their defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    pub name: String,
    pub title: String,
    precision: Precision,
    /// 1 or 2; the axes below are always three, as ROOT stores them.
    dim: usize,
    x: Axis,
    y: Axis,
    z: Axis,
    /// One cell per bin including the under- and overflows.
    contents: Vec<f64>,
    /// Squared errors, ROOT's `fSumw2`; empty when errors are not tracked.
    sumw2: Vec<f64>,
    entries: f64,
    tsumw: f64,
    tsumw2: f64,
    tsumwx: f64,
    tsumwx2: f64,
    tsumwy: f64,
    tsumwy2: f64,
    tsumwxy: f64,
    maximum: f64,
    minimum: f64,
    norm_factor: f64,
}

impl Histogram {
    fn new(name: &str, title: &str, dim: usize, x: Axis, y: Axis) -> Self {
        let ncells = if dim == 1 {
            x.nbins + 2
        } else {
            (x.nbins + 2) * (y.nbins + 2)
        };
        Self {
            name: name.to_string(),
            title: title.to_string(),
            precision: Precision::F64,
            dim,
            x,
            y,
            z: Axis::degenerate(),
            contents: vec![0.0; ncells],
            sumw2: Vec::new(),
            entries: 0.0,
            tsumw: 0.0,
            tsumw2: 0.0,
            tsumwx: 0.0,
            tsumwx2: 0.0,
            tsumwy: 0.0,
            tsumwy2: 0.0,
            tsumwxy: 0.0,
            maximum: UNSET_LIMIT,
            minimum: UNSET_LIMIT,
            norm_factor: 0.0,
        }
    }

    /// A one-dimensional histogram, `TH1D` unless told otherwise.
    pub fn h1(name: &str, title: &str, x: Axis) -> Self {
        Self::new(name, title, 1, x, Axis::degenerate())
    }

    /// A two-dimensional histogram, `TH2D` unless told otherwise.
    pub fn h2(name: &str, title: &str, x: Axis, y: Axis) -> Self {
        Self::new(name, title, 2, x, y)
    }

    /// Chooses the storage type, and with it the ROOT class written.
    ///
    /// Contents accumulated already are re-rounded to the new precision, the
    /// way `TH1::Copy` between two flavours would, so switching after filling
    /// drops whatever the new type cannot hold.
    pub fn with_precision(mut self, precision: Precision) -> Self {
        self.precision = precision;
        for c in &mut self.contents {
            *c = precision.store(*c);
        }
        self
    }

    /// Tracks the sum of squared weights, as `TH1::Sumw2` does.
    pub fn with_sumw2(mut self) -> Self {
        self.enable_sumw2();
        self
    }

    /// Sets the y-axis title: the label of the second axis of a `TH2`, and
    /// the count label of a `TH1`.
    pub fn with_y_title(mut self, title: &str) -> Self {
        self.y.title = title.to_string();
        self
    }

    pub fn with_z_title(mut self, title: &str) -> Self {
        self.z.title = title.to_string();
        self
    }

    /// `TH1::Sumw2`: existing contents seed the squared errors.
    fn enable_sumw2(&mut self) {
        self.sumw2 = self.contents.iter().map(|c| c.abs()).collect();
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn precision(&self) -> Precision {
        self.precision
    }

    pub fn x_axis(&self) -> &Axis {
        &self.x
    }

    /// The second axis. For a one-dimensional histogram this is ROOT's
    /// degenerate one-bin axis, which carries only a title.
    pub fn y_axis(&self) -> &Axis {
        &self.y
    }

    /// Cells stored, ROOT's `fNcells`: the bins plus the under- and
    /// overflows in every dimension.
    pub fn ncells(&self) -> usize {
        self.contents.len()
    }

    /// The ROOT class this histogram is written as.
    pub fn class_name(&self) -> &'static str {
        match (self.dim, self.precision) {
            (1, Precision::I8) => "TH1C",
            (1, Precision::I16) => "TH1S",
            (1, Precision::I32) => "TH1I",
            (1, Precision::I64) => "TH1L",
            (1, Precision::F32) => "TH1F",
            (1, Precision::F64) => "TH1D",
            (_, Precision::I8) => "TH2C",
            (_, Precision::I16) => "TH2S",
            (_, Precision::I32) => "TH2I",
            (_, Precision::I64) => "TH2L",
            (_, Precision::F32) => "TH2F",
            (_, Precision::F64) => "TH2D",
        }
    }

    /// The version ROOT stamps on this class. `TH1L` and `TH2L` are declared
    /// `ClassDef(…, 0)` upstream, and ROOT really does write a zero there.
    fn class_version(&self) -> u16 {
        match (self.dim, self.precision) {
            (_, Precision::I64) => 0,
            (1, _) => 3,
            (_, _) => 4,
        }
    }

    /// The global bin holding `(ix, iy)`, both counted from 1 with 0 the
    /// underflow.
    pub fn global_bin(&self, ix: usize, iy: usize) -> usize {
        iy * (self.x.nbins + 2) + ix
    }

    /// Fills a one-dimensional histogram, returning the bin hit.
    ///
    /// # Panics
    /// If the histogram is two-dimensional; use [`Histogram::fill_xy`].
    pub fn fill(&mut self, x: f64) -> usize {
        self.fill_weighted(x, 1.0)
    }

    /// Fills with a weight. A weight other than 1 starts tracking squared
    /// errors if they are not tracked already, as `TH1::Fill` does.
    ///
    /// # Panics
    /// If the histogram is two-dimensional.
    pub fn fill_weighted(&mut self, x: f64, w: f64) -> usize {
        assert_eq!(
            self.dim, 1,
            "{} is a 2-D histogram: fill it with fill_xy",
            self.name
        );
        let bin = self.x.find_bin(x);
        self.entries += 1.0;
        self.add_to_bin(bin, w);
        if bin == 0 || bin > self.x.nbins {
            return bin;
        }
        self.tsumw += w;
        self.tsumw2 += w * w;
        self.tsumwx += w * x;
        self.tsumwx2 += w * x * x;
        bin
    }

    /// Fills a two-dimensional histogram, returning the global bin hit.
    ///
    /// # Panics
    /// If the histogram is one-dimensional; use [`Histogram::fill`].
    pub fn fill_xy(&mut self, x: f64, y: f64) -> usize {
        self.fill_xy_weighted(x, y, 1.0)
    }

    /// # Panics
    /// If the histogram is one-dimensional.
    pub fn fill_xy_weighted(&mut self, x: f64, y: f64, w: f64) -> usize {
        assert_eq!(
            self.dim, 2,
            "{} is a 1-D histogram: fill it with fill",
            self.name
        );
        let ix = self.x.find_bin(x);
        let iy = self.y.find_bin(y);
        let bin = self.global_bin(ix, iy);
        self.entries += 1.0;
        self.add_to_bin(bin, w);
        // A miss on either axis is counted but excluded from the statistics.
        if ix == 0 || ix > self.x.nbins || iy == 0 || iy > self.y.nbins {
            return bin;
        }
        self.tsumw += w;
        self.tsumw2 += w * w;
        self.tsumwx += w * x;
        self.tsumwx2 += w * x * x;
        self.tsumwy += w * y;
        self.tsumwy2 += w * y * y;
        self.tsumwxy += w * x * y;
        bin
    }

    fn add_to_bin(&mut self, bin: usize, w: f64) {
        if self.sumw2.is_empty() && w != 1.0 {
            self.enable_sumw2();
        }
        self.contents[bin] = self.precision.add(self.contents[bin], w);
        if !self.sumw2.is_empty() {
            self.sumw2[bin] += w * w;
        }
    }

    pub fn entries(&self) -> f64 {
        self.entries
    }

    /// Sum of the in-range bins, matching `TH1::Integral()`.
    pub fn integral(&self) -> f64 {
        if self.dim == 1 {
            return self.contents[1..=self.x.nbins].iter().sum();
        }
        let mut sum = 0.0;
        for iy in 1..=self.y.nbins {
            for ix in 1..=self.x.nbins {
                sum += self.contents[self.global_bin(ix, iy)];
            }
        }
        sum
    }

    /// # Panics
    /// If `bin` is past [`Histogram::ncells`]. ROOT clamps instead; here an
    /// out-of-range bin is a bug, as it is for any other index.
    pub fn bin_content(&self, bin: usize) -> f64 {
        self.contents[bin]
    }

    /// `TH1::GetBinError`: the tracked error if there is one, otherwise the
    /// Poisson error ROOT assumes.
    ///
    /// # Panics
    /// If `bin` is past [`Histogram::ncells`].
    pub fn bin_error(&self, bin: usize) -> f64 {
        if self.sumw2.is_empty() {
            self.contents[bin].abs().sqrt()
        } else {
            self.sumw2[bin].sqrt()
        }
    }

    /// Whether squared errors are tracked, ROOT's `fSumw2.fN != 0`.
    pub fn has_sumw2(&self) -> bool {
        !self.sumw2.is_empty()
    }

    /// Sets a bin's content.
    ///
    /// Like `TH1::SetBinContent` this counts as an entry and discards the
    /// statistics sums, which makes ROOT recompute mean and RMS from the bins
    /// themselves.
    ///
    /// # Panics
    /// If `bin` is past [`Histogram::ncells`].
    pub fn set_bin_content(&mut self, bin: usize, content: f64) {
        self.entries += 1.0;
        self.tsumw = 0.0;
        self.contents[bin] = self.precision.store(content);
    }

    /// Sets a bin's error, tracking squared errors from here on if they were
    /// not tracked already, as `TH1::SetBinError` does.
    pub fn set_bin_error(&mut self, bin: usize, error: f64) {
        if self.sumw2.is_empty() {
            self.enable_sumw2();
        }
        self.sumw2[bin] = error * error;
    }

    /// The mean along x, from the statistics sums.
    pub fn mean(&self) -> f64 {
        if self.tsumw == 0.0 {
            0.0
        } else {
            self.tsumwx / self.tsumw
        }
    }

    /// The standard deviation along x — what ROOT calls the RMS.
    ///
    /// Mixed-sign weights can make the variance come out negative, and ROOT
    /// reports zero rather than a root of nonsense; so does this.
    pub fn std_dev(&self) -> f64 {
        if self.tsumw == 0.0 {
            return 0.0;
        }
        let mean = self.mean();
        variance_root(self.tsumwx2 / self.tsumw - mean * mean)
    }

    /// The mean along y of a two-dimensional histogram.
    pub fn mean_y(&self) -> f64 {
        if self.tsumw == 0.0 {
            0.0
        } else {
            self.tsumwy / self.tsumw
        }
    }

    /// The standard deviation along y of a two-dimensional histogram.
    pub fn std_dev_y(&self) -> f64 {
        if self.tsumw == 0.0 {
            return 0.0;
        }
        let mean = self.mean_y();
        variance_root(self.tsumwy2 / self.tsumw - mean * mean)
    }

    /// Drawing limits, ROOT's `fMaximum`/`fMinimum`; `None` when unset.
    pub fn maximum(&self) -> Option<f64> {
        (self.maximum != UNSET_LIMIT).then_some(self.maximum)
    }

    pub fn minimum(&self) -> Option<f64> {
        (self.minimum != UNSET_LIMIT).then_some(self.minimum)
    }

    pub fn set_maximum(&mut self, maximum: Option<f64>) {
        self.maximum = maximum.unwrap_or(UNSET_LIMIT);
    }

    pub fn set_minimum(&mut self, minimum: Option<f64>) {
        self.minimum = minimum.unwrap_or(UNSET_LIMIT);
    }

    // --- serialisation ----------------------------------------------------

    /// Serialises the histogram as the payload of a [`Histogram::class_name`]
    /// record.
    pub fn serialize(&self) -> Vec<u8> {
        let mut w = WBuffer::new(0);
        let outer = w.begin(self.class_version());
        let th2 = (self.dim == 2).then(|| w.begin(TH2_VERSION));
        let th1 = w.begin(TH1_VERSION);

        w.tnamed(&self.name, &self.title, K_MUST_CLEANUP);
        w.tattline(602, 1, 1);
        w.tattfill(0, 1001);
        w.tattmarker(1, 1, 1.0);

        w.i32(self.contents.len() as i32); // fNcells
        // ROOT leaves the y-axis title offset at zero and the other two at
        // one, whatever the dimension.
        write_axis(&mut w, "xaxis", &self.x, 1.0);
        write_axis(&mut w, "yaxis", &self.y, 0.0);
        write_axis(&mut w, "zaxis", &self.z, 1.0);

        w.i16(0); // fBarOffset
        w.i16(1000); // fBarWidth
        w.f64(self.entries);
        w.f64(self.tsumw);
        w.f64(self.tsumw2);
        w.f64(self.tsumwx);
        w.f64(self.tsumwx2);
        w.f64(self.maximum);
        w.f64(self.minimum);
        w.f64(self.norm_factor);
        w.array_f64(&[]); // fContour
        w.array_f64(&self.sumw2);
        w.tstring(""); // fOption
        w.empty_tlist(); // fFunctions
        w.i32(0); // fBufferSize
        w.u8(0); // fBuffer: absent
        w.i32(0); // fBinStatErrOpt
        w.i32(2); // fStatOverflows: kNeutral
        w.end(th1);

        if let Some(th2) = th2 {
            w.f64(1.0); // fScalefactor
            w.f64(self.tsumwy);
            w.f64(self.tsumwy2);
            w.f64(self.tsumwxy);
            w.end(th2);
        }

        // The concrete class's own TArray payload holds the bin contents.
        self.precision.write_array(&mut w, &self.contents);
        w.end(outer);
        w.into_vec()
    }

    /// Reads a histogram from the decompressed payload of its key, which is
    /// what names the class.
    pub fn parse(payload: &[u8], class: &str) -> Result<Self> {
        let (dim, precision) = split_class(class)?;
        let mut r = RBuffer::new(payload);

        let outer = r.class_header()?;
        let th2 = if dim == 2 {
            let h = r.class_header()?;
            if h.version != TH2_VERSION {
                return Err(Error::decode(format!(
                    "TH2 version {} is not supported (expected {TH2_VERSION})",
                    h.version
                )));
            }
            Some(h)
        } else {
            None
        };

        let th1 = r.class_header()?;
        if th1.version != TH1_VERSION {
            return Err(Error::decode(format!(
                "TH1 version {} is not supported (expected {TH1_VERSION})",
                th1.version
            )));
        }
        let (name, title) = r.tnamed()?;
        // TAttLine, TAttFill, TAttMarker
        for _ in 0..3 {
            r.skip_object()?;
        }
        let ncells = r.i32()?;
        if ncells < 0 {
            return Err(Error::decode(format!("negative cell count {ncells}")));
        }
        let x = read_axis(&mut r)?;
        let y = read_axis(&mut r)?;
        let z = read_axis(&mut r)?;

        r.skip(4)?; // fBarOffset, fBarWidth
        let entries = r.f64()?;
        let tsumw = r.f64()?;
        let tsumw2 = r.f64()?;
        let tsumwx = r.f64()?;
        let tsumwx2 = r.f64()?;
        let maximum = r.f64()?;
        let minimum = r.f64()?;
        let norm_factor = r.f64()?;

        let ncontour = r.i32()?;
        r.skip(8 * ncontour.max(0) as usize)?; // fContour
        let nsumw2 = r.i32()?;
        if nsumw2 != 0 && nsumw2 != ncells {
            return Err(Error::decode(format!(
                "fSumw2 holds {nsumw2} cells but the histogram has {ncells}"
            )));
        }
        let sumw2 = Precision::F64.read_array(&mut r, nsumw2.max(0) as usize)?;

        // fOption, fFunctions and everything after them are display state this
        // reader does not model; the byte count says where they end.
        r.seek(th1.end)?;

        let (mut tsumwy, mut tsumwy2, mut tsumwxy) = (0.0, 0.0, 0.0);
        if let Some(th2) = th2 {
            let _scalefactor = r.f64()?;
            tsumwy = r.f64()?;
            tsumwy2 = r.f64()?;
            tsumwxy = r.f64()?;
            r.seek(th2.end)?;
        }

        let n = r.i32()?;
        if n != ncells {
            return Err(Error::decode(format!(
                "{class} holds {n} bin contents but fNcells is {ncells}"
            )));
        }
        let contents = precision.read_array(&mut r, n.max(0) as usize)?;
        // The bin contents are the last member, so anything left over means
        // the payload was not the class the key claimed.
        if r.pos() != outer.end {
            return Err(Error::decode(format!(
                "{class} record ends at {} but its byte count says {}",
                r.pos(),
                outer.end
            )));
        }

        let expected = if dim == 1 {
            x.nbins + 2
        } else {
            (x.nbins + 2) * (y.nbins + 2)
        };
        if contents.len() != expected {
            return Err(Error::decode(format!(
                "{class} has {} cells but its axes describe {expected}",
                contents.len()
            )));
        }

        Ok(Self {
            name,
            title,
            precision,
            dim,
            x,
            y,
            z,
            contents,
            sumw2,
            entries,
            tsumw,
            tsumw2,
            tsumwx,
            tsumwx2,
            tsumwy,
            tsumwy2,
            tsumwxy,
            maximum,
            minimum,
            norm_factor,
        })
    }
}

/// `TH1::GetStdDev`: the root of a variance, or zero if it came out negative.
fn variance_root(variance: f64) -> f64 {
    if variance > 0.0 {
        variance.sqrt()
    } else {
        0.0
    }
}

/// Splits `TH1D` into its dimension and its storage precision.
fn split_class(class: &str) -> Result<(usize, Precision)> {
    let unsupported =
        || Error::decode(format!("{class} is not a histogram this crate can read"));
    let (dim, rest) = match class.strip_prefix("TH1") {
        Some(rest) => (1, rest),
        None => (2, class.strip_prefix("TH2").ok_or_else(unsupported)?),
    };
    let mut chars = rest.chars();
    let suffix = chars.next().ok_or_else(unsupported)?;
    if chars.next().is_some() {
        return Err(unsupported());
    }
    Ok((dim, Precision::from_suffix(suffix).ok_or_else(unsupported)?))
}

/// Writes a `TAxis` member with the attributes ROOT gives a new histogram.
fn write_axis(w: &mut WBuffer, name: &str, axis: &Axis, title_offset: f32) {
    let m = w.begin(10);
    w.tnamed(name, &axis.title, 0);

    let a = w.begin(4); // TAttAxis
    w.i32(510); // fNdivisions
    w.i16(1); // fAxisColor
    w.i16(1); // fLabelColor
    w.i16(42); // fLabelFont
    w.f32(0.005); // fLabelOffset
    w.f32(0.035); // fLabelSize
    w.f32(0.03); // fTickLength
    w.f32(title_offset);
    w.f32(0.035); // fTitleSize
    w.i16(1); // fTitleColor
    w.i16(42); // fTitleFont
    w.end(a);

    w.i32(axis.nbins as i32);
    w.f64(axis.min);
    w.f64(axis.max);
    w.array_f64(&axis.edges); // fXbins: empty when the binning is uniform
    w.i32(0); // fFirst
    w.i32(0); // fLast
    w.u16(0); // fBits2
    w.u8(0); // fTimeDisplay
    w.tstring(""); // fTimeFormat
    w.null_ref(); // fLabels
    w.null_ref(); // fModLabs
    w.end(m);
}

fn read_axis(r: &mut RBuffer) -> Result<Axis> {
    let h = r.class_header()?;
    let (_name, title) = r.tnamed()?;
    r.skip_object()?; // TAttAxis
    let nbins = r.i32()?;
    let min = r.f64()?;
    let max = r.f64()?;
    let nedges = r.i32()?;
    let edges = Precision::F64.read_array(r, nedges.max(0) as usize)?;
    // fFirst onwards is the drawn range and the labels, which the byte count
    // lets us skip whatever the TAxis version.
    r.seek(h.end)?;

    if nbins < 1 {
        return Err(Error::decode(format!("axis has {nbins} bins")));
    }
    let mut axis = if edges.is_empty() {
        Axis::uniform(nbins as usize, min, max)?
    } else {
        if edges.len() != nbins as usize + 1 {
            return Err(Error::decode(format!(
                "axis has {nbins} bins but {} edges",
                edges.len()
            )));
        }
        Axis::variable(&edges)?
    };
    axis.title = title;
    Ok(axis)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The binning of the analysis histogram.
    fn hist() -> Histogram {
        Histogram::h1(
            "h",
            "t",
            Axis::uniform(110, 70.0, 180.0).unwrap().titled("x"),
        )
        .with_y_title("y")
        .with_sumw2()
    }

    /// The edges probed against `TAxis::FindBin` in ROOT 6.38.
    fn var_axis() -> Axis {
        Axis::variable(&[0.0, 1.0, 3.0, 7.0, 10.0, 20.0]).unwrap()
    }

    #[test]
    fn bins_follow_root_find_bin_conventions() {
        let h = hist();
        let x = h.x_axis();
        assert_eq!(x.find_bin(69.9), 0, "below range is underflow");
        assert_eq!(x.find_bin(70.0), 1, "lower edge lands in the first bin");
        assert_eq!(x.find_bin(70.5), 1);
        assert_eq!(x.find_bin(71.0), 2);
        assert_eq!(x.find_bin(179.999), 110);
        assert_eq!(x.find_bin(180.0), 111, "upper edge is overflow");
        assert_eq!(x.find_bin(1e6), 111);
        assert_eq!(x.find_bin(f64::NAN), 111, "NaN lands in the overflow");
    }

    #[test]
    fn variable_bins_find_the_same_bins_root_does() {
        // Values and answers taken from ROOT 6.38 with these edges.
        let a = var_axis();
        assert_eq!(a.nbins(), 5);
        assert_eq!(a.min(), 0.0);
        assert_eq!(a.max(), 20.0);
        for (x, bin) in [
            (-1.0, 0),
            (0.0, 1),
            (0.5, 1),
            (1.0, 2),
            (3.0, 3),
            (19.9, 5),
            (20.0, 6),
            (25.0, 6),
        ] {
            assert_eq!(a.find_bin(x), bin, "bin of {x}");
        }
        assert_eq!(a.edge(1), 0.0);
        assert_eq!(a.edge(6), 20.0);
        assert_eq!(a.bin_width(2), 2.0);
        assert_eq!(a.bin_center(2), 2.0);
        assert!(!a.is_uniform());
    }

    #[test]
    fn uniform_axis_edges_are_computed_not_stored() {
        let a = Axis::uniform(4, 0.0, 2.0).unwrap();
        assert!(a.is_uniform());
        assert_eq!(a.edges(), vec![0.0, 0.5, 1.0, 1.5, 2.0]);
        assert_eq!(a.bin_center(1), 0.25);
        assert_eq!(a.bin_width(3), 0.5);
    }

    #[test]
    fn malformed_axes_are_rejected() {
        assert!(Axis::uniform(0, 0.0, 1.0).is_err(), "no bins");
        assert!(Axis::uniform(10, 1.0, 1.0).is_err(), "empty range");
        assert!(Axis::uniform(10, 2.0, 1.0).is_err(), "inverted range");
        assert!(Axis::uniform(10, 0.0, f64::INFINITY).is_err());
        assert!(Axis::variable(&[1.0]).is_err(), "one edge is no bin");
        assert!(Axis::variable(&[0.0, 1.0, 1.0]).is_err(), "repeated edge");
        assert!(Axis::variable(&[0.0, 2.0, 1.0]).is_err(), "unsorted");
    }

    #[test]
    fn out_of_range_fills_count_as_entries_but_not_statistics() {
        let mut h = hist();
        h.fill(125.0);
        h.fill(10.0);
        h.fill(500.0);
        assert_eq!(h.entries(), 3.0);
        assert_eq!(h.integral(), 1.0, "only the in-range fill counts");
        assert_eq!(h.tsumw, 1.0);
        assert_eq!(h.tsumwx, 125.0);
        assert_eq!(h.bin_content(0), 1.0, "underflow");
        assert_eq!(h.bin_content(111), 1.0, "overflow");
    }

    #[test]
    fn sumw2_tracks_unweighted_contents() {
        let mut h = hist();
        for _ in 0..5 {
            h.fill(100.0);
        }
        let bin = h.x_axis().find_bin(100.0);
        assert_eq!(h.bin_content(bin), 5.0);
        assert_eq!(h.sumw2[bin], 5.0);
        assert_eq!(h.bin_error(bin), 5f64.sqrt());
    }

    #[test]
    fn errors_are_poisson_until_sumw2_is_asked_for() {
        let mut h = Histogram::h1("h", "t", Axis::uniform(1, 0.0, 1.0).unwrap());
        h.fill(0.5);
        h.fill(0.5);
        assert!(!h.has_sumw2());
        assert_eq!(h.bin_error(1), 2f64.sqrt());
    }

    #[test]
    fn a_weighted_fill_starts_tracking_errors_from_the_contents_so_far() {
        // ROOT 6.38: after Fill(x) then Fill(x, 2.0) the error is sqrt(1+4).
        let mut h = Histogram::h1("h", "t", Axis::uniform(1, 0.0, 1.0).unwrap());
        h.fill(0.5);
        assert!(!h.has_sumw2(), "an unweighted fill leaves fSumw2 empty");
        h.fill_weighted(0.5, 2.0);
        assert!(h.has_sumw2());
        assert_eq!(h.bin_error(1), 5f64.sqrt());
        assert_eq!(h.bin_content(1), 3.0);
        assert_eq!(h.entries(), 2.0, "entries count fills, not weight");
        assert_eq!(h.integral(), 3.0);
    }

    #[test]
    fn a_weight_of_exactly_one_does_not_start_tracking_errors() {
        let mut h = Histogram::h1("h", "t", Axis::uniform(1, 0.0, 1.0).unwrap());
        h.fill_weighted(0.5, 1.0);
        assert!(!h.has_sumw2(), "ROOT only reacts to weights other than 1");
    }

    #[test]
    fn mean_and_std_dev_match_root() {
        // ROOT 6.38, TH1D(10, 0, 10) filled at 1, 2 and 8.
        let mut h = Histogram::h1("m", "", Axis::uniform(10, 0.0, 10.0).unwrap());
        h.fill(1.0);
        h.fill(2.0);
        h.fill(8.0);
        assert_eq!(h.mean(), 3.6666666666666665);
        assert_eq!(h.std_dev(), 3.091206165165235);
    }

    #[test]
    fn a_negative_variance_reports_zero_spread_as_root_does() {
        // ROOT 6.38 gives this exact mean and a standard deviation of 0, the
        // variance being -24.296685545870794.
        let mut h = Histogram::h1("h", "", Axis::uniform(3, 0.0, 3.0).unwrap());
        for _ in 0..200 {
            h.fill(0.5);
        }
        h.fill_weighted(1.5, 0.6);
        h.fill_weighted(2.5, -300.0);
        assert_eq!(h.mean(), 6.530181086519114);
        assert_eq!(h.std_dev(), 0.0);
    }

    #[test]
    fn statistics_of_an_empty_histogram_are_zero_not_nan() {
        let h = hist();
        assert_eq!(h.mean(), 0.0);
        assert_eq!(h.std_dev(), 0.0);
        assert_eq!(h.integral(), 0.0);
    }

    // --- precision --------------------------------------------------------

    #[test]
    fn integer_contents_saturate_the_way_root_does() {
        // Each case was run through ROOT 6.38 first.
        let mut c = Histogram::h1("c", "", Axis::uniform(1, 0.0, 1.0).unwrap())
            .with_precision(Precision::I8);
        for _ in 0..200 {
            c.fill(0.5);
        }
        assert_eq!(c.bin_content(1), 127.0, "TH1C saturates at 127");
        assert_eq!(c.entries(), 200.0, "entries keep counting past saturation");

        let mut c = Histogram::h1("c", "", Axis::uniform(1, 0.0, 1.0).unwrap())
            .with_precision(Precision::I8);
        c.fill_weighted(0.5, -300.0);
        assert_eq!(c.bin_content(1), -127.0);

        let mut s = Histogram::h1("s", "", Axis::uniform(1, 0.0, 1.0).unwrap())
            .with_precision(Precision::I16);
        s.fill_weighted(0.5, 40_000.0);
        assert_eq!(s.bin_content(1), 32_767.0);

        let mut i = Histogram::h1("i", "", Axis::uniform(1, 0.0, 1.0).unwrap())
            .with_precision(Precision::I32);
        i.fill_weighted(0.5, 3e9);
        assert_eq!(i.bin_content(1), 2_147_483_647.0);
    }

    #[test]
    fn integer_contents_truncate_the_weight_towards_zero() {
        // ROOT 6.38: two fills of weight 0.6 leave a TH1C empty, and two of
        // 2.5 leave a TH1I at 4.
        let mut c = Histogram::h1("c", "", Axis::uniform(1, 0.0, 1.0).unwrap())
            .with_precision(Precision::I8);
        c.fill_weighted(0.5, 0.6);
        c.fill_weighted(0.5, 0.6);
        assert_eq!(c.bin_content(1), 0.0);

        let mut i = Histogram::h1("i", "", Axis::uniform(1, 0.0, 1.0).unwrap())
            .with_precision(Precision::I32);
        i.fill_weighted(0.5, 2.5);
        i.fill_weighted(0.5, 2.5);
        assert_eq!(i.bin_content(1), 4.0);
        i.fill_weighted(0.5, -2.5);
        assert_eq!(i.bin_content(1), 2.0);

        // The statistics sums stay in double precision regardless.
        assert_eq!(i.tsumw, 2.5);
    }

    #[test]
    fn float_contents_round_where_root_rounds() {
        // ROOT 6.38: a TH1F filled with 1e-9 and then three times with 1
        // holds exactly 3, while a TH1D holds 3.000000001.
        let mut f = Histogram::h1("f", "", Axis::uniform(1, 0.0, 1.0).unwrap())
            .with_precision(Precision::F32);
        let mut d = Histogram::h1("d", "", Axis::uniform(1, 0.0, 1.0).unwrap());
        for h in [&mut f, &mut d] {
            h.fill_weighted(0.5, 1e-9);
            for _ in 0..3 {
                h.fill(0.5);
            }
        }
        assert_eq!(f.bin_content(1), 3.0);
        assert_eq!(d.bin_content(1), 3.000000001);

        // The weight is rounded to float before the addition, so this one
        // stays at 1 instead of stepping up to 1 + 2^-23.
        let mut f = Histogram::h1("f", "", Axis::uniform(1, 0.0, 1.0).unwrap())
            .with_precision(Precision::F32);
        f.fill_weighted(0.5, 1.0);
        f.fill_weighted(0.5, 2f64.powi(-24) + 2f64.powi(-50));
        assert_eq!(f.bin_content(1), 1.0);
    }

    #[test]
    fn class_names_cover_both_dimensions_and_every_precision() {
        let axis = || Axis::uniform(2, 0.0, 1.0).unwrap();
        let names: Vec<&str> = [
            Precision::I8,
            Precision::I16,
            Precision::I32,
            Precision::I64,
            Precision::F32,
            Precision::F64,
        ]
        .iter()
        .map(|&p| Histogram::h1("h", "", axis()).with_precision(p).class_name())
        .collect();
        assert_eq!(names, ["TH1C", "TH1S", "TH1I", "TH1L", "TH1F", "TH1D"]);

        let h2 = Histogram::h2("h", "", axis(), axis()).with_precision(Precision::F32);
        assert_eq!(h2.class_name(), "TH2F");
        assert_eq!(split_class("TH2F").unwrap(), (2, Precision::F32));
        assert!(split_class("TProfile").is_err());
        assert!(split_class("TH3D").is_err());
        assert!(split_class("TH1DD").is_err());
        assert!(split_class("TH1").is_err());
    }

    // --- two dimensions ---------------------------------------------------

    #[test]
    fn two_dimensional_fills_follow_roots_cell_numbering() {
        let mut h = Histogram::h2(
            "h2",
            "",
            Axis::uniform(3, 0.0, 3.0).unwrap(),
            Axis::uniform(2, 0.0, 2.0).unwrap(),
        );
        assert_eq!(h.ncells(), 20, "(3 + 2) x (2 + 2)");
        let bin = h.fill_xy(1.5, 0.5);
        assert_eq!(bin, h.global_bin(2, 1));
        assert_eq!(bin, 7);
        assert_eq!(h.bin_content(7), 1.0);
        assert_eq!(h.entries(), 1.0);
        assert_eq!(h.integral(), 1.0);
        // The sums ROOT reports for this single fill.
        assert_eq!((h.tsumwx, h.tsumwx2), (1.5, 2.25));
        assert_eq!((h.tsumwy, h.tsumwy2, h.tsumwxy), (0.5, 0.25, 0.75));
        assert_eq!(h.mean(), 1.5);
        assert_eq!(h.mean_y(), 0.5);
    }

    #[test]
    fn a_miss_on_either_axis_is_left_out_of_the_statistics() {
        let mut h = Histogram::h2(
            "h2",
            "",
            Axis::uniform(2, 0.0, 2.0).unwrap(),
            Axis::uniform(2, 0.0, 2.0).unwrap(),
        );
        h.fill_xy(0.5, 0.5);
        h.fill_xy(0.5, 9.0); // y overflow, x in range
        h.fill_xy(-1.0, 0.5); // x underflow, y in range
        assert_eq!(h.entries(), 3.0);
        assert_eq!(h.integral(), 1.0);
        assert_eq!(h.tsumw, 1.0);
        assert_eq!(h.bin_content(h.global_bin(1, 3)), 1.0, "y overflow row");
        assert_eq!(h.bin_content(h.global_bin(0, 1)), 1.0, "x underflow column");
    }

    #[test]
    #[should_panic(expected = "2-D histogram")]
    fn filling_a_two_dimensional_histogram_with_one_value_is_a_bug() {
        let axis = || Axis::uniform(2, 0.0, 1.0).unwrap();
        Histogram::h2("h", "", axis(), axis()).fill(0.5);
    }

    #[test]
    #[should_panic(expected = "1-D histogram")]
    fn filling_a_one_dimensional_histogram_with_two_values_is_a_bug() {
        Histogram::h1("h", "", Axis::uniform(2, 0.0, 1.0).unwrap()).fill_xy(0.5, 0.5);
    }

    // --- setters ----------------------------------------------------------

    #[test]
    fn setting_a_bin_discards_the_statistics_as_root_does() {
        let mut h = hist();
        h.fill(125.0);
        h.set_bin_content(3, 7.0);
        assert_eq!(h.bin_content(3), 7.0);
        assert_eq!(h.tsumw, 0.0, "ROOT invalidates the sums");
        assert_eq!(h.entries(), 2.0, "and counts the change as an entry");
    }

    #[test]
    fn setting_a_bin_of_an_integer_histogram_truncates() {
        let mut h = Histogram::h1("h", "", Axis::uniform(3, 0.0, 3.0).unwrap())
            .with_precision(Precision::I32);
        h.set_bin_content(1, 2.9);
        assert_eq!(h.bin_content(1), 2.0);
    }

    #[test]
    fn setting_an_error_starts_tracking_them() {
        let mut h = Histogram::h1("h", "", Axis::uniform(3, 0.0, 3.0).unwrap());
        h.fill(0.5);
        assert!(!h.has_sumw2());
        h.set_bin_error(2, 4.0);
        assert!(h.has_sumw2());
        assert_eq!(h.bin_error(2), 4.0);
        assert_eq!(h.bin_error(1), 1.0, "seeded from the existing contents");
    }

    // --- serialisation ----------------------------------------------------

    #[test]
    fn serialisation_is_readable_and_self_consistent() {
        let mut h = hist();
        h.fill(125.0);
        let bytes = h.serialize();
        let mut r = RBuffer::new(&bytes);
        let outer = r.class_header().unwrap();
        assert_eq!(outer.version, 3);
        assert_eq!(outer.end, bytes.len(), "byte count spans the whole record");
        let th1 = r.class_header().unwrap();
        assert_eq!(th1.version, 8);
        assert_eq!(r.tnamed().unwrap(), ("h".to_string(), "t".to_string()));
    }

    #[test]
    fn a_two_dimensional_record_nests_th2_between_the_class_and_th1() {
        let axis = || Axis::uniform(2, 0.0, 1.0).unwrap();
        let h = Histogram::h2("h", "t", axis(), axis());
        let bytes = h.serialize();
        let mut r = RBuffer::new(&bytes);
        assert_eq!(r.class_header().unwrap().version, 4, "TH2D");
        let th2 = r.class_header().unwrap();
        assert_eq!(th2.version, 5, "TH2");
        assert_eq!(r.class_header().unwrap().version, 8, "TH1");
        // TH2 ends where the bin contents begin, which is not the record end.
        assert!(th2.end < bytes.len());
    }

    #[test]
    fn the_long_flavours_are_stamped_version_zero_like_roots_own() {
        let h = Histogram::h1("h", "t", Axis::uniform(2, 0.0, 1.0).unwrap())
            .with_precision(Precision::I64);
        let bytes = h.serialize();
        assert_eq!(RBuffer::new(&bytes).class_header().unwrap().version, 0);
    }

    /// Every combination of dimension, precision and binning survives a trip
    /// through the ROOT record format unchanged.
    #[test]
    fn every_flavour_round_trips_through_its_own_serialisation() {
        let precisions = [
            Precision::I8,
            Precision::I16,
            Precision::I32,
            Precision::I64,
            Precision::F32,
            Precision::F64,
        ];
        for precision in precisions {
            for variable in [false, true] {
                let x = if variable {
                    var_axis().titled("x")
                } else {
                    Axis::uniform(5, 0.0, 20.0).unwrap().titled("x")
                };

                let mut h1 = Histogram::h1("h1", "one dimension", x.clone())
                    .with_precision(precision)
                    .with_y_title("counts")
                    .with_sumw2();
                for v in [-1.0, 0.5, 0.5, 2.0, 8.0, 19.0, 25.0] {
                    h1.fill(v);
                }
                let back = Histogram::parse(&h1.serialize(), h1.class_name()).unwrap();
                assert_eq!(back, h1, "{} {variable}", h1.class_name());
                assert_eq!(back.x_axis().title, "x");
                assert_eq!(back.y_axis().title, "counts");
                assert_eq!(back.x_axis().is_uniform(), !variable);

                let mut h2 = Histogram::h2(
                    "h2",
                    "two dimensions",
                    x.clone(),
                    Axis::uniform(3, -1.0, 2.0).unwrap().titled("y"),
                )
                .with_precision(precision)
                .with_z_title("counts");
                for (a, b) in [(0.5, 0.0), (2.0, 1.5), (8.0, -5.0), (25.0, 0.5)] {
                    h2.fill_xy(a, b);
                }
                let back = Histogram::parse(&h2.serialize(), h2.class_name()).unwrap();
                assert_eq!(back, h2, "{} {variable}", h2.class_name());
                assert!(!back.has_sumw2(), "an untracked fSumw2 stays untracked");
            }
        }
    }

    #[test]
    fn drawing_limits_survive_a_round_trip() {
        let mut h = hist();
        assert_eq!(h.maximum(), None);
        h.set_maximum(Some(12.0));
        h.set_minimum(Some(-3.0));
        let back = Histogram::parse(&h.serialize(), h.class_name()).unwrap();
        assert_eq!(back.maximum(), Some(12.0));
        assert_eq!(back.minimum(), Some(-3.0));
    }

    #[test]
    fn an_array_longer_than_the_record_is_rejected_before_it_is_allocated() {
        let data = [0u8; 16];
        let mut r = RBuffer::new(&data);
        assert!(Precision::F64.read_array(&mut r, 1_000_000_000).is_err());
        assert!(Precision::I8.read_array(&mut r, 17).is_err());
        assert_eq!(Precision::F64.read_array(&mut r, 2).unwrap(), [0.0, 0.0]);
    }

    #[test]
    fn a_content_count_that_disagrees_with_fncells_is_rejected() {
        let mut h = hist();
        h.fill(125.0);
        let mut bytes = h.serialize();
        // The bin contents are last, so their count sits just in front of
        // them; claim there are a billion of them.
        let count = bytes.len() - 4 - 8 * h.ncells();
        bytes[count..count + 4].copy_from_slice(&1_000_000_000i32.to_be_bytes());
        let err = Histogram::parse(&bytes, "TH1D").unwrap_err().to_string();
        assert!(err.contains("1000000000"), "unhelpful message: {err}");
    }

    #[test]
    fn parsing_the_wrong_class_for_a_payload_fails_cleanly() {
        let h = hist();
        let bytes = h.serialize();
        // A TH1F payload is shorter than a TH1D one, so the contents run out.
        assert!(Histogram::parse(&bytes, "TH1F").is_err());
        assert!(Histogram::parse(&bytes, "TH2D").is_err());
        assert!(Histogram::parse(&bytes[..20], "TH1D").is_err());
        assert!(Histogram::parse(&[], "TH1D").is_err());
    }
}

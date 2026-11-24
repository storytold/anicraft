//! Sound analysis for animation (ADR 0114): how loud a sound is on each frame of the movie,
//! and which mouth shape its speech calls for there.
//!
//! * [`levels`] is the amplitude envelope: the RMS of the samples each movie frame plays.
//!   [`normalised`] scales it to the loudest frame and [`shape`] gates and smooths it, which is
//!   what Convert Audio to Keyframes maps onto a property.
//! * [`visemes`] is auto lip-sync's analysis. It is built from public phonetics only: the
//!   first two formants tell the vowels apart (Peterson and Barney, 1952), so each voiced
//!   frame is sorted by where its spectrum's energy lies into a small set of mouth shapes
//!   ([`Viseme`]). No speech recognition, no trained model, no data: a few band sums on a
//!   short Fourier transform. It is a first pass for the animator to polish.
//!
//! Everything here is a pure function of its inputs (same sound, same answer), and the work
//! is bounded however long the sound is: at most [`MAX_ANALYSIS_FRAMES`] movie frames, a
//! capped number of samples read for each.

use crate::Pcm;

/// Most movie frames one analysis covers (the timeline's own limit).
pub const MAX_ANALYSIS_FRAMES: usize = 100_000;
/// Widest smoothing, in frames either side.
pub const MAX_SMOOTHING: u32 = 24;
/// Samples [`levels`] reads at most, over the whole analysis (longer stretches are sampled
/// evenly).
const LEVEL_BUDGET: usize = 96 * 1024 * 1024;

/// Which channel's loudness is measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Both together (a mono sound's one channel).
    #[default]
    Both,
    Left,
    Right,
}

impl Channel {
    pub const ALL: [Channel; 3] = [Channel::Both, Channel::Left, Channel::Right];

    pub fn key(self) -> &'static str {
        match self {
            Channel::Both => "both",
            Channel::Left => "left",
            Channel::Right => "right",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Channel::Both => "Both channels",
            Channel::Left => "Left channel",
            Channel::Right => "Right channel",
        }
    }

    pub fn from_key(s: &str) -> Option<Channel> {
        let s = s.trim().to_ascii_lowercase();
        Channel::ALL.into_iter().find(|c| c.key() == s)
    }
}

/// The stretch of a sound an analysis covers, in movie frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    /// The movie's frame rate.
    pub fps: f32,
    /// Movie frames from the sound's start to the first frame analysed.
    pub offset: u32,
    /// Movie frames analysed (at most [`MAX_ANALYSIS_FRAMES`]).
    pub frames: u32,
    /// Frames of the sound (at its own rate) heard from its start, repeats included: it plays
    /// round and round up to here and is silent after. `u64::MAX` for a sound on a loop.
    pub heard: u64,
}

/// A sound read by movie frame.
struct Source<'a> {
    pcm: &'a Pcm,
    len: u64,
    heard: u64,
    /// Sound frames per movie frame.
    per: f64,
    offset: f64,
    frames: usize,
}

impl<'a> Source<'a> {
    fn new(pcm: &'a Pcm, span: &Span) -> Source<'a> {
        let fps = if span.fps.is_finite() && span.fps > 0.0 { f64::from(span.fps) } else { 24.0 };
        Source { pcm, len: pcm.frames() as u64, heard: span.heard, per: f64::from(pcm.rate.max(1)) / fps, offset: f64::from(span.offset), frames: (span.frames as usize).min(MAX_ANALYSIS_FRAMES) }
    }

    /// The sound frame where analysed frame `i` (fractions allowed) starts.
    fn at(&self, i: f64) -> i64 {
        ((self.offset + i) * self.per).round().clamp(0.0, 4.0e18) as i64
    }

    /// Sound frame `i` as it is heard (left, right): repeats wrap, silence outside.
    fn frame(&self, i: i64) -> (f32, f32) {
        match u64::try_from(i) {
            Ok(i) if i < self.heard && self.len > 0 => self.pcm.frame((i % self.len) as usize),
            _ => (0.0, 0.0),
        }
    }
}

/// How loud the sound is on each movie frame of `span`: the RMS of the samples that frame
/// plays, of one channel or both. `0` where nothing sounds.
pub fn levels(pcm: &Pcm, span: &Span, channel: Channel) -> Vec<f32> {
    let src = Source::new(pcm, span);
    // Long frames (a low frame rate, a high sample rate) are sampled evenly.
    let cap = (LEVEL_BUDGET / src.frames.max(1)).max(64) as i64;
    (0..src.frames)
        .map(|i| {
            let (a, b) = (src.at(i as f64), src.at(i as f64 + 1.0));
            let b = b.max(a.saturating_add(1));
            let stride = ((b - a + cap - 1) / cap).max(1);
            let (mut sum, mut n) = (0.0f64, 0u32);
            let mut j = a;
            while j < b {
                let (l, r) = src.frame(j);
                let (l, r) = (f64::from(l), f64::from(r));
                sum += match channel {
                    Channel::Both => (l * l + r * r) * 0.5,
                    Channel::Left => l * l,
                    Channel::Right => r * r,
                };
                n += 1;
                j = j.saturating_add(stride);
            }
            let v = (sum / f64::from(n.max(1))).sqrt() as f32;
            if v.is_finite() { v } else { 0.0 }
        })
        .collect()
}

/// `levels` as shares of the loudest one (`0..=1`); silence stays silence.
pub fn normalised(levels: &[f32]) -> Vec<f32> {
    let peak = levels.iter().copied().filter(|v| v.is_finite()).fold(0.0f32, f32::max);
    if peak <= 1e-9 {
        return vec![0.0; levels.len()];
    }
    levels.iter().map(|v| if v.is_finite() { (v / peak).clamp(0.0, 1.0) } else { 0.0 }).collect()
}

/// Normalised levels gated and smoothed, still `0..=1`:
///
/// * **threshold** (`0..1`): frames quieter than this are silent, and the rest are stretched
///   over the room above it, so the value rises from zero as the sound crosses the threshold
///   (background noise is ignored, and there is no jump at the gate);
/// * **smoothing** (frames, at most [`MAX_SMOOTHING`]): each frame becomes the mean of itself
///   and that many frames either side. Centred, so a beat stays on its frame instead of
///   lagging behind it.
pub fn shape(levels: &[f32], threshold: f32, smoothing: u32) -> Vec<f32> {
    let th = if threshold.is_finite() { threshold.clamp(0.0, 0.99) } else { 0.0 };
    let gated: Vec<f32> = levels
        .iter()
        .map(|&v| {
            let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
            if v <= th || v <= 0.0 { 0.0 } else { (v - th) / (1.0 - th) }
        })
        .collect();
    let r = smoothing.min(MAX_SMOOTHING) as usize;
    if r == 0 || gated.len() < 2 {
        return gated;
    }
    // Running sums: the mean of the frames within `r` of each (fewer at the ends).
    let mut sums = Vec::with_capacity(gated.len() + 1);
    let mut acc = 0.0f64;
    sums.push(acc);
    for v in &gated {
        acc += f64::from(*v);
        sums.push(acc);
    }
    (0..gated.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(r), (i + r + 1).min(gated.len()));
            let total = sums.get(b).copied().unwrap_or(acc) - sums.get(a).copied().unwrap_or(0.0);
            ((total / (b - a).max(1) as f64) as f32).clamp(0.0, 1.0)
        })
        .collect()
}

/// A mouth shape of the small set auto lip-sync tells apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Viseme {
    /// Nothing is said: the mouth at rest.
    Rest,
    /// An open jaw: "ah", "a" as in father or cat.
    Open,
    /// Spread lips: "ee", "e", and the hiss of s, t and their kin.
    Wide,
    /// Rounded lips: "oh", "oo", w.
    Round,
    /// Lips together: m, b, p, and the short stops between syllables.
    Closed,
}

impl Viseme {
    pub const ALL: [Viseme; 5] = [Viseme::Rest, Viseme::Open, Viseme::Wide, Viseme::Round, Viseme::Closed];

    pub fn key(self) -> &'static str {
        match self {
            Viseme::Rest => "rest",
            Viseme::Open => "open",
            Viseme::Wide => "wide",
            Viseme::Round => "round",
            Viseme::Closed => "closed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Viseme::Rest => "Rest",
            Viseme::Open => "Open",
            Viseme::Wide => "Wide",
            Viseme::Round => "Round",
            Viseme::Closed => "Closed",
        }
    }

    /// What it stands for, for a tooltip.
    pub fn hint(self) -> &'static str {
        match self {
            Viseme::Rest => "Silence: the mouth at rest",
            Viseme::Open => "An open jaw: ah, as in father and cat",
            Viseme::Wide => "Spread lips: ee and e, and the hiss of s and t",
            Viseme::Round => "Rounded lips: oh, oo and w",
            Viseme::Closed => "Lips together: m, b, p and the short stops between syllables",
        }
    }

    pub fn from_key(s: &str) -> Option<Viseme> {
        let s = s.trim().to_ascii_lowercase();
        Viseme::ALL.into_iter().find(|v| v.key() == s)
    }

    /// Its place in [`Viseme::ALL`].
    pub fn index(self) -> usize {
        Viseme::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }
}

/// Twelve studio mouth shapes (Adobe Animate auto lip-sync parity, P-054):
/// `neutral`, `ah`, `d`, `ee`, `f`, `l`, `m`, `oh`, `r`, `s`, `uh`, `woo`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Viseme12 {
    /// Silence: the mouth relaxed at rest.
    #[default]
    Neutral,
    /// Wide open jaw: "ah", "a" as in father or cat.
    Ah,
    /// Alveolar consonant/tap: d, t, n, k, g.
    D,
    /// Spread front vowel: "ee", "i", "e" as in beet or bet.
    Ee,
    /// Labiodental fricative: f, v (lower lip against upper teeth).
    F,
    /// Lateral tongue lift: l, th.
    L,
    /// Pressed lips: m, b, p, and brief inter-syllable closures.
    M,
    /// Rounded mid-back vowel: "oh", "o" as in boat or bought.
    Oh,
    /// Retroflex approximant: r, er.
    R,
    /// Alveolar sibilant hiss: s, z, sh, ch.
    S,
    /// Relaxed central vowel: "uh" as in cup or about.
    Uh,
    /// Tightly pursed lips: "oo", "w" as in boot or woo.
    Woo,
}

impl Viseme12 {
    pub const ALL: [Viseme12; 12] = [
        Viseme12::Neutral,
        Viseme12::Ah,
        Viseme12::D,
        Viseme12::Ee,
        Viseme12::F,
        Viseme12::L,
        Viseme12::M,
        Viseme12::Oh,
        Viseme12::R,
        Viseme12::S,
        Viseme12::Uh,
        Viseme12::Woo,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Viseme12::Neutral => "neutral",
            Viseme12::Ah => "ah",
            Viseme12::D => "d",
            Viseme12::Ee => "ee",
            Viseme12::F => "f",
            Viseme12::L => "l",
            Viseme12::M => "m",
            Viseme12::Oh => "oh",
            Viseme12::R => "r",
            Viseme12::S => "s",
            Viseme12::Uh => "uh",
            Viseme12::Woo => "woo",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Viseme12::Neutral => "Neutral",
            Viseme12::Ah => "Ah",
            Viseme12::D => "D",
            Viseme12::Ee => "Ee",
            Viseme12::F => "F",
            Viseme12::L => "L",
            Viseme12::M => "M",
            Viseme12::Oh => "Oh",
            Viseme12::R => "R",
            Viseme12::S => "S",
            Viseme12::Uh => "Uh",
            Viseme12::Woo => "Woo",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Viseme12::Neutral => "Silence: the mouth relaxed at rest",
            Viseme12::Ah => "Open jaw: ah and a, as in father and cat",
            Viseme12::D => "Alveolar stop or tap: d, t, n, k, g",
            Viseme12::Ee => "Spread front lips: ee, i and e",
            Viseme12::F => "Lower lip to upper teeth: f and v",
            Viseme12::L => "Tongue tip behind upper teeth: l and th",
            Viseme12::M => "Lips pressed together: m, b, p",
            Viseme12::Oh => "Rounded open lips: oh and o",
            Viseme12::R => "Curled tongue and rounded corners: r",
            Viseme12::S => "Clenched sibilant hiss: s, z, sh, ch",
            Viseme12::Uh => "Relaxed mid-open mouth: uh, as in cup",
            Viseme12::Woo => "Tightly pursed lips: oo and w",
        }
    }

    pub fn from_key(s: &str) -> Option<Viseme12> {
        let s = s.trim().to_ascii_lowercase();
        if s == "rest" {
            return Some(Viseme12::Neutral);
        }
        Viseme12::ALL.into_iter().find(|v| v.key() == s)
    }

    pub fn index(self) -> usize {
        Viseme12::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }
}

/// How auto lip-sync reads a sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LipOptions {
    /// Frames quieter than this share of the loudest frame (`0..1`) are silence.
    pub threshold: f32,
    /// Fewest frames a mouth shape is held (shorter ones join their neighbour), so the mouth
    /// doesn't flicker. Lips together is kept however short: a closure is short by nature.
    pub hold: u32,
}

/// Longest hold.
pub const MAX_HOLD: u32 = 12;

impl Default for LipOptions {
    fn default() -> Self {
        LipOptions { threshold: 0.08, hold: 2 }
    }
}

/// Samples in a spectrum's window (a power of two), at the analysis rate.
const WINDOW: usize = 512;
/// The analysis rate aimed for (Hz): sounds are thinned to between this and twice this, which
/// keeps the first three formants and makes the window 32 to 64 ms long.
const ANALYSIS_RATE: u32 = 8_000;
/// Source frames averaged into one analysis sample, at most.
const MAX_TAPS: u32 = 4;
/// The band the vowels' first formants and the back vowels' second lie in (Hz).
const LOW: (f32, f32) = (200.0, 1_400.0);
/// The band the front vowels' second formants and the fricatives' hiss lie in (Hz).
const HIGH: (f32, f32) = (1_600.0, 3_600.0);
/// With pre-emphasis, the share of energy in [`HIGH`] from which a frame is a front vowel
/// (its second formant is up there), and from which it is mostly hiss.
const FRONT_SHARE: f32 = 0.25;
const HISS_SHARE: f32 = 0.6;
/// The centre of the low band's energy (Hz) from which the jaw is open: "ah" has its first
/// formant near 730 Hz and its second near 1100, "oh" near 570 and 840, "oo" near 300 and 870.
const OPEN_CENTRE: f32 = 715.0;
/// The same for a frame with a high second formant: "a" as in cat (660 Hz) against "e" (530).
const OPEN_FRONT_CENTRE: f32 = 620.0;
/// A weak frame whose energy sits below this (Hz) is a hum through closed lips (m, n).
const HUM_CENTRE: f32 = 400.0;
const HUM_LEVEL: f32 = 0.2;
/// A silence between sounds no longer than this (seconds) is a closure, not a rest.
const CLOSURE_SECONDS: f32 = 0.12;

/// The mouth shape each movie frame of `span` calls for.
///
/// A frame quieter than the threshold is silent: [`Viseme::Closed`] when it is a short gap
/// inside speech, [`Viseme::Rest`] otherwise. A sounding frame is sorted by its spectrum (a
/// Hann window of [`WINDOW`] samples about the frame's middle): mostly hiss or a high second
/// formant with a low first is [`Viseme::Wide`]; a first formant high up is [`Viseme::Open`];
/// energy low down is [`Viseme::Round`], or [`Viseme::Closed`] when it is a weak hum.
pub fn visemes(pcm: &Pcm, span: &Span, opts: &LipOptions) -> Vec<Viseme> {
    let src = Source::new(pcm, span);
    let level = normalised(&levels(pcm, span, Channel::Both));
    let threshold = if opts.threshold.is_finite() { opts.threshold.clamp(0.0, 0.99) } else { 0.0 };
    let spectrum = Spectrum::new(pcm.rate);
    let mut out: Vec<Option<Viseme>> = level.iter().enumerate().map(|(i, &v)| (v > threshold && v > 0.0).then(|| spectrum.classify(&src, i, v))).collect();
    // Silences: a short one between sounds is the lips closing, the rest are rests.
    let fps = if span.fps.is_finite() && span.fps > 0.0 { span.fps } else { 24.0 };
    let closure = ((CLOSURE_SECONDS * fps).round() as usize).max(1);
    let mut i = 0;
    while i < out.len() {
        if out.get(i).is_some_and(Option::is_some) {
            i += 1;
            continue;
        }
        let end = out.iter().skip(i).position(Option::is_some).map_or(out.len(), |n| i + n);
        let inside = i > 0 && end < out.len();
        let fill = if inside && end - i <= closure { Viseme::Closed } else { Viseme::Rest };
        for slot in out.iter_mut().take(end).skip(i) {
            *slot = Some(fill);
        }
        i = end;
    }
    held(out.into_iter().map(|v| v.unwrap_or(Viseme::Rest)).collect(), opts.hold.clamp(1, MAX_HOLD) as usize)
}

/// Twelve-viseme studio speech classification (P-054): deterministic frame-by-frame mapping
/// into [`Viseme12`] from RMS loudness, zero-crossing rate (sibilants `s` vs labiodentals `f`),
/// formant band centroids (`ah`, `uh`, `oh`, `woo`, `ee`, `l`, `r`, `d`, `m`), and envelope
/// onset slope (`d`/`l`/`m`), smoothed to `opts.hold` minimum frames while keeping brief
/// bilabial closures (`m`).
pub fn visemes_12(pcm: &Pcm, span: &Span, opts: &LipOptions) -> Vec<Viseme12> {
    let src = Source::new(pcm, span);
    let level = normalised(&levels(pcm, span, Channel::Both));
    let threshold = if opts.threshold.is_finite() { opts.threshold.clamp(0.0, 0.99) } else { 0.0 };
    let spectrum = Spectrum::new(pcm.rate);
    let mut out: Vec<Option<Viseme12>> = Vec::with_capacity(level.len());
    for (i, &v) in level.iter().enumerate() {
        if v > threshold && v > 0.0 {
            let prev = i.checked_sub(1).and_then(|p| level.get(p)).copied().unwrap_or(v);
            let onset_slope = v - prev;
            out.push(Some(spectrum.classify_12(&src, i, v, onset_slope)));
        } else {
            out.push(None);
        }
    }
    let fps = if span.fps.is_finite() && span.fps > 0.0 { span.fps } else { 24.0 };
    let closure = ((CLOSURE_SECONDS * fps).round() as usize).max(1);
    let mut i = 0;
    while i < out.len() {
        if out.get(i).is_some_and(Option::is_some) {
            i += 1;
            continue;
        }
        let end = out.iter().skip(i).position(Option::is_some).map_or(out.len(), |n| i + n);
        let inside = i > 0 && end < out.len();
        let fill = if inside && end - i <= closure { Viseme12::M } else { Viseme12::Neutral };
        for slot in out.iter_mut().take(end).skip(i) {
            *slot = Some(fill);
        }
        i = end;
    }
    held_12(out.into_iter().map(|v| v.unwrap_or(Viseme12::Neutral)).collect(), opts.hold.clamp(1, MAX_HOLD) as usize)
}

fn held_12(mut v: Vec<Viseme12>, hold: usize) -> Vec<Viseme12> {
    if hold <= 1 {
        return v;
    }
    let mut i = 0;
    let mut before: Option<Viseme12> = None;
    while i < v.len() {
        let Some(&cur) = v.get(i) else { break };
        let end = v.iter().skip(i).position(|x| *x != cur).map_or(v.len(), |n| i + n);
        let short = end - i < hold && cur != Viseme12::M;
        let open = |x: &Viseme12| *x != Viseme12::M;
        let join = if short { before.filter(open).or_else(|| v.get(end).copied().filter(open)) } else { None };
        match join {
            Some(to) if to != cur => {
                for slot in v.iter_mut().take(end).skip(i) {
                    *slot = to;
                }
                before = Some(to);
            }
            _ => before = Some(cur),
        }
        i = end;
    }
    v
}

/// `v` with runs shorter than `hold` frames joined to the run before them (the first to the one
/// after). Closed runs are kept however short.
fn held(mut v: Vec<Viseme>, hold: usize) -> Vec<Viseme> {
    if hold <= 1 {
        return v;
    }
    let mut i = 0;
    let mut before: Option<Viseme> = None;
    while i < v.len() {
        let Some(&cur) = v.get(i) else { break };
        let end = v.iter().skip(i).position(|x| *x != cur).map_or(v.len(), |n| i + n);
        let short = end - i < hold && cur != Viseme::Closed;
        // What a short run becomes: the run before it, or the one after at the start and after
        // a closure (lips together don't stay together for the sound that follows them).
        let open = |x: &Viseme| *x != Viseme::Closed;
        let join = if short { before.filter(open).or_else(|| v.get(end).copied().filter(open)) } else { None };
        match join {
            Some(to) if to != cur => {
                for slot in v.iter_mut().take(end).skip(i) {
                    *slot = to;
                }
                before = Some(to);
            }
            _ => before = Some(cur),
        }
        i = end;
    }
    v
}

/// The short Fourier transform's tables for one sound.
struct Spectrum {
    /// Source frames per analysis sample, and how many of them are averaged.
    thin: u32,
    taps: u32,
    /// Hz per bin.
    bin: f32,
    hann: Vec<f32>,
    cos: Vec<f32>,
    sin: Vec<f32>,
    /// Each bin's gain under pre-emphasis (`y[n] = x[n] - 0.95 x[n-1]`): about 6 dB an octave
    /// up, which evens out the voice's falling spectrum so the formants can be compared.
    emphasis: Vec<f32>,
}

impl Spectrum {
    fn new(rate: u32) -> Spectrum {
        let thin = (rate / ANALYSIS_RATE).max(1);
        let n = WINDOW as f32;
        let turn = std::f32::consts::TAU / n;
        Spectrum {
            thin,
            taps: thin.min(MAX_TAPS),
            bin: rate as f32 / thin as f32 / n,
            hann: (0..WINDOW).map(|i| 0.5 - 0.5 * (turn * i as f32).cos()).collect(),
            cos: (0..WINDOW / 2).map(|i| (turn * i as f32).cos()).collect(),
            sin: (0..WINDOW / 2).map(|i| (turn * i as f32).sin()).collect(),
            emphasis: (0..WINDOW / 2).map(|k| 1.9025 - 1.9 * (turn * k as f32).cos()).collect(),
        }
    }

    /// The mouth shape of analysed frame `i`, which sounds at `level` of the loudest frame.
    fn classify(&self, src: &Source, i: usize, level: f32) -> Viseme {
        // The window, about the frame's middle, thinned to the analysis rate.
        let thin = i64::from(self.thin);
        let start = src.at(i as f64 + 0.5).saturating_sub(thin * (WINDOW as i64) / 2);
        let step = (self.thin / self.taps).max(1);
        let mut re: Vec<f32> = self
            .hann
            .iter()
            .enumerate()
            .map(|(j, w)| {
                let at = start.saturating_add(j as i64 * thin);
                let sum: f32 = (0..self.taps).map(|t| src.frame(at.saturating_add(i64::from(t * step)))).map(|(l, r)| (l + r) * 0.5).sum();
                w * sum / self.taps as f32
            })
            .collect();
        let mut im = vec![0.0f32; WINDOW];
        fft(&mut re, &mut im, &self.cos, &self.sin);
        // Band sums over the power spectrum.
        let (mut low, mut low_weighted, mut low_emph, mut high_emph) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for (k, ((r, m), e)) in re.iter().zip(&im).zip(&self.emphasis).enumerate() {
            let f = k as f32 * self.bin;
            let p = r * r + m * m;
            if f >= LOW.0 && f < LOW.1 {
                low += p;
                low_weighted += p * f;
                low_emph += p * e;
            } else if f >= HIGH.0 && f < HIGH.1 {
                high_emph += p * e;
            }
        }
        let total = low_emph + high_emph;
        if !total.is_finite() || total <= 0.0 {
            return Viseme::Closed;
        }
        let front = high_emph / total;
        let centre = if low > 0.0 { low_weighted / low } else { 0.0 };
        if front >= HISS_SHARE {
            Viseme::Wide
        } else if front >= FRONT_SHARE {
            if centre >= OPEN_FRONT_CENTRE { Viseme::Open } else { Viseme::Wide }
        } else if centre >= OPEN_CENTRE {
            Viseme::Open
        } else if level < HUM_LEVEL && centre < HUM_CENTRE {
            Viseme::Closed
        } else {
            Viseme::Round
        }
    }

    /// Classifies frame `i` into one of the 12 studio visemes ([`Viseme12`]).
    fn classify_12(&self, src: &Source, i: usize, level: f32, onset_slope: f32) -> Viseme12 {
        let thin = i64::from(self.thin);
        let start = src.at(i as f64 + 0.5).saturating_sub(thin * (WINDOW as i64) / 2);
        let step = (self.thin / self.taps).max(1);
        let mut raw_samples = Vec::with_capacity(WINDOW);
        let mut re = Vec::with_capacity(WINDOW);
        for (j, w) in self.hann.iter().enumerate() {
            let at = start.saturating_add(j as i64 * thin);
            let sum: f32 = (0..self.taps)
                .map(|t| src.frame(at.saturating_add(i64::from(t * step))))
                .map(|(l, r)| (l + r) * 0.5)
                .sum();
            let s = sum / self.taps as f32;
            raw_samples.push(s);
            re.push(w * s);
        }
        // Time-domain zero-crossing rate (Hz) across the analysis window.
        let mut crossings = 0u32;
        for pair in raw_samples.windows(2) {
            if let [a, b] = pair
                && (a.abs() + b.abs()) > 1e-4
                && ((*a >= 0.0) != (*b >= 0.0))
            {
                crossings += 1;
            }
        }
        let eff_rate = self.bin * WINDOW as f32;
        let zcr_hz = crossings as f32 * 0.5 * eff_rate / (WINDOW.saturating_sub(1).max(1) as f32);

        let mut im = vec![0.0f32; WINDOW];
        fft(&mut re, &mut im, &self.cos, &self.sin);

        let (mut f1_pow, mut f1_weighted, mut high_pow) = (0.0f32, 0.0f32, 0.0f32);
        let (mut low_emph, mut high_emph) = (0.0f32, 0.0f32);
        let (mut mid_emph, mut r_emph, mut d_emph, mut ee_emph) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let (mut fric_f_emph, mut fric_s_emph) = (0.0f32, 0.0f32);

        for (k, ((r, m), e)) in re.iter().zip(&im).zip(&self.emphasis).enumerate() {
            let f = k as f32 * self.bin;
            let p = r * r + m * m;
            let pe = p * e;
            if (180.0..950.0).contains(&f) {
                f1_pow += p;
                f1_weighted += p * f;
            }
            if (LOW.0..LOW.1).contains(&f) {
                low_emph += pe;
            } else if (1_400.0..3_950.0).contains(&f) {
                high_pow += p;
                high_emph += pe;
            }
            if (1_030.0..1_480.0).contains(&f) {
                mid_emph += pe;
            } else if (1_480.0..1_800.0).contains(&f) {
                r_emph += pe;
            } else if (1_800.0..2_120.0).contains(&f) {
                d_emph += pe;
            } else if (2_120.0..3_200.0).contains(&f) {
                ee_emph += pe;
            }
            if (1_700.0..2_650.0).contains(&f) {
                fric_f_emph += pe;
            } else if (2_650.0..3_950.0).contains(&f) {
                fric_s_emph += pe;
            }
        }

        let total = low_emph + high_emph;
        if !total.is_finite() || total <= 0.0 {
            return Viseme12::M;
        }
        let high_share = high_emph / total;
        let voiced_share = f1_pow / (f1_pow + high_pow).max(1e-12);
        let f1_centre = if f1_pow > 0.0 { f1_weighted / f1_pow } else { 0.0 };

        // 1. Unvoiced & lightly-voiced fricatives (`s`/`z`/`sh` vs softer labiodental `f`/`v`).
        if high_share >= 0.54 && voiced_share < 0.35 {
            return if fric_s_emph >= fric_f_emph * 0.85 || zcr_hz >= 2_850.0 {
                Viseme12::S
            } else {
                Viseme12::F
            };
        }
        if zcr_hz >= 2_100.0 && high_share >= 0.42 && voiced_share < 0.35 && level < 0.42 {
            return if zcr_hz >= 3_000.0 { Viseme12::S } else { Viseme12::F };
        }

        // 2. Weak bilabial hum (`m`, `b`, `p` murmur).
        if level < 0.22 && f1_centre < 400.0 && high_share < 0.16 {
            return Viseme12::M;
        }

        // 3. Alveolar stop/tap (`d`, `t`, `n`): sharp burst onset or strong alveolar locus (~1.9 kHz).
        if (onset_slope >= 0.24 && high_share >= 0.30 && f1_centre < 620.0 && d_emph >= ee_emph * 0.5)
            || (f1_centre < 460.0 && d_emph > ee_emph && d_emph > r_emph && (d_emph / total) >= 0.22)
        {
            return Viseme12::D;
        }

        // 4. Open jaw (`ah`, `a` as in father/cat).
        if f1_centre >= 645.0 {
            return Viseme12::Ah;
        }

        // 5. Spread front vowels (`ee`, `i`, `e`).
        if (ee_emph / total) >= 0.20 || (f1_centre >= 470.0 && high_share >= 0.24) {
            return Viseme12::Ee;
        }

        // 6. Retroflex `r` (characteristic lowered F3 in 1.5..1.8 kHz).
        if (r_emph / total) >= 0.18 && f1_centre < 560.0 {
            return Viseme12::R;
        }

        // 7. Lateral `l` (low F1 + clear mid-band F2 around 1.1..1.45 kHz).
        if f1_centre < 465.0 && (mid_emph / total) >= 0.22 {
            return Viseme12::L;
        }

        // 8. Central mid-open `uh` (mid F1 + central F2).
        if f1_centre >= 465.0 && ((mid_emph + r_emph) / total) >= 0.20 {
            return Viseme12::Uh;
        }

        // 9. Back rounded `oh` vs tightly pursed `woo`.
        if f1_centre >= 465.0 {
            Viseme12::Oh
        } else {
            Viseme12::Woo
        }
    }
}

/// An in-place radix-2 fast Fourier transform (Cooley and Tukey, 1965) of `re + i·im`, whose
/// length is a power of two; `cos` and `sin` hold a turn's first half in steps of that length.
fn fft(re: &mut [f32], im: &mut [f32], cos: &[f32], sin: &[f32]) {
    let n = re.len();
    if n < 2 || !n.is_power_of_two() || im.len() != n || cos.len() < n / 2 || sin.len() < n / 2 {
        return;
    }
    // Bit-reversed order.
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let (half, step) = (len / 2, n / len);
        for (cr, ci) in re.chunks_exact_mut(len).zip(im.chunks_exact_mut(len)) {
            let (ar, br) = cr.split_at_mut(half);
            let (ai, bi) = ci.split_at_mut(half);
            let turn = cos.iter().step_by(step).zip(sin.iter().step_by(step));
            for ((((ar, br), ai), bi), (c, s)) in ar.iter_mut().zip(br).zip(ai).zip(bi).zip(turn) {
                let (tr, ti) = (*br * c + *bi * s, *bi * c - *br * s);
                (*br, *bi) = (*ar - tr, *ai - ti);
                (*ar, *ai) = (*ar + tr, *ai + ti);
            }
        }
        len *= 2;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: u32 = 22_050;
    const FPS: f32 = 24.0;

    /// `seconds` of sines: (Hz, amplitude) summed.
    fn tone(parts: &[(f32, f32)], seconds: f32) -> Vec<f32> {
        (0..(seconds * RATE as f32) as usize).map(|i| parts.iter().map(|(f, a)| a * (TAU * f * i as f32 / RATE as f32).sin()).sum()).collect()
    }

    fn silence(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * RATE as f32) as usize]
    }

    fn pcm(samples: Vec<f32>) -> Pcm {
        Pcm::new(RATE, 1, samples).unwrap()
    }

    fn whole(p: &Pcm) -> Span {
        Span { fps: FPS, offset: 0, frames: (p.seconds() * f64::from(FPS)).ceil() as u32, heard: p.frames() as u64 }
    }

    /// A sine burst is loud exactly on the frames it sounds on, and silent on the others.
    #[test]
    fn a_burst_is_loud_exactly_where_it_sounds() {
        // Frames 0..12 silent, 12..18 a 440 Hz burst at 0.5, 18..24 silent.
        let mut s = silence(0.5);
        s.extend(tone(&[(440.0, 0.5)], 0.25));
        s.extend(silence(0.25));
        let p = pcm(s);
        let l = levels(&p, &whole(&p), Channel::Both);
        assert_eq!(l.len(), 24);
        for (i, v) in l.iter().enumerate() {
            if (12..18).contains(&i) {
                assert!((v - 0.5 / 2f32.sqrt()).abs() < 0.01, "frame {i}: the RMS of a sine at 0.5, got {v}");
            } else {
                assert_eq!(*v, 0.0, "frame {i} is silent");
            }
        }
        let n = normalised(&l);
        assert!(n[12..18].iter().all(|v| *v > 0.98) && n[..12].iter().all(|v| *v == 0.0));
        // Silence stays silence, whatever it is divided by.
        assert_eq!(normalised(&[0.0, 0.0]), vec![0.0, 0.0]);
        assert_eq!(normalised(&[f32::NAN, 0.5]), vec![0.0, 1.0]);
    }

    #[test]
    fn channels_are_measured_apart_and_an_offset_starts_inside_the_sound() {
        // Left: a tone for the first half second. Right: for the second.
        let n = RATE as usize;
        let t = tone(&[(330.0, 0.8)], 0.5);
        let mut s = vec![0.0f32; 2 * n];
        for i in 0..n / 2 {
            s[2 * i] = t[i];
            s[2 * (n / 2 + i) + 1] = t[i];
        }
        let p = Pcm::new(RATE, 2, s).unwrap();
        let span = Span { fps: FPS, offset: 0, frames: 24, heard: p.frames() as u64 };
        let (l, r, both) = (levels(&p, &span, Channel::Left), levels(&p, &span, Channel::Right), levels(&p, &span, Channel::Both));
        assert!(l[..12].iter().all(|v| *v > 0.5) && l[12..].iter().all(|v| *v == 0.0));
        assert!(r[..12].iter().all(|v| *v == 0.0) && r[12..].iter().all(|v| *v > 0.5));
        assert!(both.iter().all(|v| *v > 0.35), "one side sounding is heard in both");
        // From frame 12 on: the right channel sounds from the first frame analysed.
        let late = Span { offset: 12, frames: 6, ..span };
        assert!(levels(&p, &late, Channel::Right).iter().all(|v| *v > 0.5));
        assert_eq!(levels(&p, &late, Channel::Left), vec![0.0; 6]);
        // Past what is heard it is silent; a repeat plays it again.
        let twice = Span { offset: 24, frames: 12, heard: 2 * p.frames() as u64, ..span };
        assert!(levels(&p, &twice, Channel::Left).iter().all(|v| *v > 0.5), "the second play");
        assert_eq!(levels(&p, &Span { offset: 24, frames: 4, ..span }, Channel::Both), vec![0.0; 4]);
    }

    #[test]
    fn the_threshold_gates_and_smoothing_spreads_without_moving_the_beat() {
        let l = [0.0, 0.05, 0.1, 1.0, 0.1, 0.05, 0.0];
        // No threshold, no smoothing: unchanged.
        assert_eq!(shape(&l, 0.0, 0), l.to_vec());
        // Below the threshold is silence; above it starts from zero.
        let g = shape(&l, 0.1, 0);
        assert_eq!(g, vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        let g = shape(&[0.5, 0.75, 1.0], 0.5, 0);
        assert!(g[0] == 0.0 && (g[1] - 0.5).abs() < 1e-6 && g[2] == 1.0);
        // Smoothing is centred: the peak stays on its frame, and its sides are equal.
        let s = shape(&l, 0.1, 1);
        let peak = s.iter().copied().fold(0.0, f32::max);
        assert_eq!(s[3], peak);
        assert!((s[2] - s[4]).abs() < 1e-6 && s[2] > 0.0 && s[1] == 0.0);
        assert!((s[3] - 1.0 / 3.0).abs() < 1e-6);
        // Out-of-range settings are brought into range.
        assert_eq!(shape(&l, f32::NAN, 0), l.to_vec());
        assert!(shape(&l, 5.0, 1_000_000).iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
        assert!(shape(&[], 0.2, 3).is_empty());
    }

    /// Vowels as their first two formants (Peterson and Barney's averages), the second weaker
    /// as in a voice, land on the mouth shapes animators draw for them.
    #[test]
    fn vowels_made_of_their_formants_get_their_mouths() {
        type Case = (&'static str, &'static [(f32, f32)], Viseme);
        let cases: [Case; 8] = [
            ("ah (father)", &[(730.0, 0.6), (1090.0, 0.3)], Viseme::Open),
            ("a (cat)", &[(660.0, 0.6), (1720.0, 0.2)], Viseme::Open),
            ("ee (beet)", &[(270.0, 0.6), (2290.0, 0.15)], Viseme::Wide),
            ("e (bet)", &[(530.0, 0.6), (1840.0, 0.2)], Viseme::Wide),
            ("oh (bought)", &[(570.0, 0.6), (840.0, 0.3)], Viseme::Round),
            ("oo (boot)", &[(300.0, 0.6), (870.0, 0.2)], Viseme::Round),
            ("s (hiss)", &[(2900.0, 0.3), (3300.0, 0.3)], Viseme::Wide),
            ("uh (but)", &[(640.0, 0.6), (1190.0, 0.4)], Viseme::Open),
        ];
        for (name, parts, want) in cases {
            let p = pcm(tone(parts, 0.5));
            let v = visemes(&p, &whole(&p), &LipOptions::default());
            assert_eq!(v.len(), 12);
            assert!(v.iter().all(|x| *x == want), "{name}: {v:?}");
        }
    }

    #[test]
    fn silence_rests_short_gaps_close_and_a_hum_is_lips_together() {
        // Half a second of rest, "ah", a two-frame gap, "ee", a hum, then rest.
        let mut s = silence(0.5);
        s.extend(tone(&[(730.0, 0.6), (1090.0, 0.3)], 0.25));
        s.extend(silence(2.0 / FPS));
        s.extend(tone(&[(270.0, 0.6), (2290.0, 0.15)], 0.25));
        s.extend(tone(&[(250.0, 0.07)], 0.25));
        s.extend(silence(0.5));
        let p = pcm(s);
        let v = visemes(&p, &whole(&p), &LipOptions { threshold: 0.05, hold: 2 });
        let runs: Vec<(Viseme, usize)> = v.chunk_by(|a, b| a == b).map(|r| (r[0], r.len())).collect();
        let kinds: Vec<Viseme> = runs.iter().map(|r| r.0).collect();
        assert_eq!(kinds, vec![Viseme::Rest, Viseme::Open, Viseme::Closed, Viseme::Wide, Viseme::Closed, Viseme::Rest], "{runs:?}");
        assert_eq!(runs[0].1, 12, "the mouth opens on the frame the voice starts");
        assert_eq!(runs[2].1, 2, "the gap");
        // A long silence inside speech is a rest, not a closure.
        let mut s = tone(&[(730.0, 0.6)], 0.25);
        s.extend(silence(0.5));
        s.extend(tone(&[(730.0, 0.6)], 0.25));
        let p = pcm(s);
        let v = visemes(&p, &whole(&p), &LipOptions::default());
        assert!(v[6..18].iter().all(|x| *x == Viseme::Rest), "{v:?}");
        // A wholly silent sound is all rest.
        let p = pcm(silence(0.5));
        assert!(visemes(&p, &whole(&p), &LipOptions::default()).iter().all(|x| *x == Viseme::Rest));
    }

    #[test]
    fn short_mouths_join_their_neighbour_but_closures_stay() {
        use Viseme::*;
        assert_eq!(held(vec![Open, Open, Wide, Open, Open], 2), vec![Open; 5]);
        assert_eq!(held(vec![Wide, Open, Open, Open], 2), vec![Open; 4], "the first joins what follows");
        assert_eq!(held(vec![Open, Open, Closed, Wide, Wide], 2), vec![Open, Open, Closed, Wide, Wide]);
        assert_eq!(held(vec![Open, Open, Closed, Wide, Round, Round], 2), vec![Open, Open, Closed, Round, Round, Round], "after a closure, the sound that follows");
        assert_eq!(held(vec![Closed, Wide, Closed], 2), vec![Closed, Wide, Closed], "between closures it stays");
        assert_eq!(held(vec![Open, Wide, Round, Open], 1), vec![Open, Wide, Round, Open]);
        assert_eq!(held(vec![Open, Open, Open, Wide, Wide, Round], 3), vec![Open; 6]);
        assert_eq!(held(vec![], 3), vec![]);
        assert_eq!(held(vec![Wide], 4), vec![Wide]);
    }

    /// Same sound, same answer; and odd input is answered without a panic.
    #[test]
    fn analysis_is_deterministic_and_bounded() {
        let mut s = tone(&[(730.0, 0.6), (1090.0, 0.3)], 0.4);
        s.extend(tone(&[(300.0, 0.4), (870.0, 0.1)], 0.4));
        let p = pcm(s);
        let span = whole(&p);
        assert_eq!(levels(&p, &span, Channel::Both), levels(&p, &span, Channel::Both));
        assert_eq!(visemes(&p, &span, &LipOptions::default()), visemes(&p, &span, &LipOptions::default()));
        // A frame rate that isn't one, an offset past the end, no frames, an endless loop.
        for span in [
            Span { fps: f32::NAN, offset: 0, frames: 5, heard: 10 },
            Span { fps: 0.001, offset: u32::MAX, frames: 3, heard: u64::MAX },
            Span { fps: 1.0e9, offset: 7, frames: 0, heard: 0 },
            Span { fps: 24.0, offset: 0, frames: u32::MAX, heard: 1 },
        ] {
            let l = levels(&p, &span, Channel::Left);
            assert!(l.len() <= MAX_ANALYSIS_FRAMES && l.iter().all(|v| v.is_finite()));
        }
        let odd = Span { fps: 0.5, offset: 3, frames: 40, heard: u64::MAX };
        assert_eq!(visemes(&p, &odd, &LipOptions { threshold: f32::NAN, hold: 99 }).len(), 40);
        // A looped sound keeps sounding past its length.
        let looped = Span { fps: FPS, offset: 100, frames: 10, heard: u64::MAX };
        assert!(levels(&p, &looped, Channel::Both).iter().all(|v| *v > 0.1));
    }

    #[test]
    fn the_transform_finds_a_tone_in_its_bin() {
        let n = 64usize;
        let turn = TAU / n as f32;
        let (cos, sin): (Vec<f32>, Vec<f32>) = (0..n / 2).map(|i| ((turn * i as f32).cos(), (turn * i as f32).sin())).unzip();
        let mut re: Vec<f32> = (0..n).map(|i| (turn * 5.0 * i as f32).cos()).collect();
        let mut im = vec![0.0f32; n];
        fft(&mut re, &mut im, &cos, &sin);
        for k in 0..n {
            let mag = (re[k] * re[k] + im[k] * im[k]).sqrt();
            let want = if k == 5 || k == n - 5 { n as f32 / 2.0 } else { 0.0 };
            assert!((mag - want).abs() < 1e-3, "bin {k}: {mag}");
        }
        // Lengths that aren't a power of two are left alone rather than read out of bounds.
        let mut odd = vec![1.0f32; 6];
        fft(&mut odd, &mut [0.0; 6], &cos, &sin);
        assert_eq!(odd, vec![1.0; 6]);
    }

    #[test]
    fn keys_name_channels_and_mouths() {
        for c in Channel::ALL {
            assert_eq!(Channel::from_key(c.key()), Some(c));
        }
        for v in Viseme::ALL {
            assert_eq!(Viseme::from_key(&v.key().to_uppercase()), Some(v));
            assert_eq!(Viseme::ALL[v.index()], v);
        }
        for v in Viseme12::ALL {
            assert_eq!(Viseme12::from_key(&v.key().to_uppercase()), Some(v));
            assert_eq!(Viseme12::ALL[v.index()], v);
            assert!(!v.label().is_empty() && !v.hint().is_empty());
        }
        assert_eq!(Viseme12::from_key("rest"), Some(Viseme12::Neutral));
        assert_eq!(Channel::from_key("centre"), None);
        assert_eq!(Viseme::from_key("smile"), None);
        assert_eq!(Viseme12::from_key("smile"), None);
    }

    #[test]
    fn studio_12_visemes_distinguish_all_twelve_mouth_shapes() {
        type Case = (&'static str, &'static [(f32, f32)], Viseme12);
        let cases: [Case; 10] = [
            ("ah", &[(730.0, 0.6), (1090.0, 0.3)], Viseme12::Ah),
            ("d", &[(330.0, 0.5), (1950.0, 0.28)], Viseme12::D),
            ("ee", &[(270.0, 0.6), (2290.0, 0.18)], Viseme12::Ee),
            ("f", &[(1950.0, 0.25), (2350.0, 0.25)], Viseme12::F),
            ("l", &[(360.0, 0.6), (1280.0, 0.30)], Viseme12::L),
            ("oh", &[(570.0, 0.6), (840.0, 0.30)], Viseme12::Oh),
            ("r", &[(420.0, 0.6), (1640.0, 0.24)], Viseme12::R),
            ("s", &[(2950.0, 0.3), (3350.0, 0.3)], Viseme12::S),
            ("uh", &[(560.0, 0.6), (1190.0, 0.35)], Viseme12::Uh),
            ("woo", &[(300.0, 0.6), (870.0, 0.2)], Viseme12::Woo),
        ];
        for (name, parts, want) in cases {
            let p = pcm(tone(parts, 0.5));
            let v = visemes_12(&p, &whole(&p), &LipOptions::default());
            assert_eq!(v.len(), 12);
            assert!(v.iter().all(|x| *x == want), "{name}: got {v:?}, expected {want:?}");
        }
        // Silence -> Neutral, short gap -> M, weak low hum -> M.
        let mut s = silence(0.5);
        s.extend(tone(&[(730.0, 0.6), (1090.0, 0.3)], 0.25));
        s.extend(silence(2.0 / FPS));
        s.extend(tone(&[(270.0, 0.6), (2290.0, 0.18)], 0.25));
        s.extend(tone(&[(250.0, 0.07)], 0.25));
        s.extend(silence(0.5));
        let p = pcm(s);
        let v = visemes_12(&p, &whole(&p), &LipOptions { threshold: 0.05, hold: 2 });
        let kinds: Vec<Viseme12> = v.chunk_by(|a, b| a == b).map(|r| r[0]).collect();
        assert_eq!(
            kinds,
            vec![Viseme12::Neutral, Viseme12::Ah, Viseme12::M, Viseme12::Ee, Viseme12::M, Viseme12::Neutral]
        );
    }
}

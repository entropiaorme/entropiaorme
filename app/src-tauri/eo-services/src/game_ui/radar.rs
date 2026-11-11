//! Reading the radar: where it is, the coordinates printed under it, and
//! how far it reaches.
//!
//! The radar is not a window but part of the heads-up display: a green
//! map disc inside a dark rim, with the player as a white dot at its
//! centre. The player can drag it anywhere and size it, so it is found
//! from what it is, never from where it usually sits: a large green
//! region whose edge, seen from inside, is a circle met by a dark rim all
//! the way round, with the dot at its centre. A green scene, a progress
//! bar or the skill bars in a window are green but not round, not
//! rimmed, or have no dot.
//!
//! The coordinates are printed under the disc straight onto the game
//! scene, `LON` aligned with the disc's left edge and `LAT` with its
//! right edge. Each is looked for only in its half of the band below the
//! disc, must be the one piece of lettering there, must lie wholly inside
//! the band and clear of any window, and must read as its own label and
//! one run of digits. Each value then checks its own width: the label is
//! printed the same at every size, so it and the space after it measure
//! the font's digit pitch, and the digits' ink must be as wide as that many digits. A
//! reading a digit short or long fails that, and so does lettering widened
//! by the scene behind it; both are refused. The value must also lie in
//! the coordinate range of the planet the caller says the player is on:
//! a value cut short at its end by something drawn over it that is not a
//! window shrinks its ink and its reading together, which no width can
//! show. A digit lost leaves a number about a tenth the size, outside the
//! range of every planet whose coordinates do not start near zero; where
//! a range does, such a value can only be refused by its width. When both
//! values read, their widths are compared with each other as well.
//!
//! The rings drawn inside the disc are labelled with their distances.
//! Their radii, found from the brightness round the centre, and the
//! distances read from their labels give the map's scale; the radii must
//! be in the proportion of the distances read, or no scale is given.

use serde::Deserialize;

use crate::game_ui::geometry::BgrImage;

use super::field::{read_run, Recogniser};
use super::geometry::{chroma, Pixels, Rect, TURN_DEGREES};
use super::profile::UiProfile;
use super::text::{scenery_runs, stroke_ink, StrokeProfile, TextProfile, TextRun};
use super::validate::{distance_labels, labelled_integer};
use super::window::find_windows;

/// What the radar looks like, as the profile describes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadarProfile {
    /// The map disc and its rim.
    pub disc: DiscProfile,
    /// The range rings inside the disc.
    pub rings: RingProfile,
    /// The coordinates printed under the disc.
    pub coordinates: CoordinatesProfile,
    /// The planet's name printed above the disc.
    pub planet: PlanetLabelProfile,
    /// A reader that found the radar captures only round it next time:
    /// the disc, the coordinates under it and the name above it, with
    /// this much more on every side, as a share of the radius.
    pub neighbourhood_margin: f64,
}

/// The planet's name, printed over the scene centred above the disc.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanetLabelProfile {
    /// The band holding the name, from and to, in shares of the disc's
    /// radius above the disc's highest point.
    pub band: [f64; 2],
    /// How far the band reaches either side of the disc's centre, as a
    /// share of the radius.
    pub reach: f64,
    /// The name is centred over the disc: its middle lies within this
    /// share of the radius of the disc's centre.
    pub centred: f64,
    /// Rows above and below the lettering that a window may not reach
    /// into.
    pub guard_rows: i32,
    /// How the lettering's strokes group into runs.
    pub lettering: TextProfile,
    /// How the lettering is told from the scene behind it.
    pub stroke: StrokeProfile,
}

/// The map disc, its rim and the player's dot.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscProfile {
    /// A pixel of the map is at least this green,
    pub min_green: u8,
    /// and greener than it is red and blue by at least these.
    pub green_over_red: u8,
    pub green_over_blue: u8,
    /// The frame is searched in square cells of this many pixels a side;
    /// a cell is map when this share of its pixels are.
    pub cell: i32,
    pub cell_share: f32,
    /// The disc's radius, in pixels, at the smallest and largest the
    /// radar can be sized to.
    pub radius: [f64; 2],
    /// Walking out from the centre, the map ends where a pixel stops
    /// being greener than red and blue by this much
    pub edge_green_margin: u8,
    /// and the next this many pixels are no greener,
    pub edge_run: i32,
    /// with at least this share of them as dark as the rim: no brighter
    /// than `rim_max_lum`. Lettering and lines on the map are thinner.
    pub rim_max_lum: u8,
    pub rim_share: f32,
    /// Rays cast from the centre to find the edge.
    pub rays: usize,
    /// The radii a ray searches for the edge, as shares of the radius
    /// first estimated: a window over part of the disc leaves that
    /// estimate short, or its centre off the disc's.
    pub ray_span: [f64; 2],
    /// An edge point is on the circle within this share of the radius.
    pub fit_tolerance: f64,
    /// The circle is tried through three edge points a fixed share of
    /// the points apart, one division of the circle per entry: with 3,
    /// points a third of the way round from each other. Several spacings
    /// keep some triples clear of an arc that a window or the rim's
    /// buttons hide.
    pub fit_divisions: Vec<usize>,
    /// Least-squares refinements of the best circle over the points that
    /// agree with it.
    pub fit_passes: usize,
    /// Least share of the rays whose edge is on the circle. The rest
    /// end on the buttons set into the rim, or on a window in front.
    pub min_on_circle: f64,
    /// Widest angle, in degrees, between neighbouring points on the
    /// circle: the circle must be seen most of the way round.
    pub max_arc_gap: f64,
    /// The player's dot at the centre.
    pub dot: DotProfile,
}

/// The white dot that marks the player at the radar's centre.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotProfile {
    /// A pixel of the dot has every channel at least this bright
    pub min_channel: u8,
    /// and is a grey, its channels within this of each other.
    pub max_chroma: u8,
    /// How far from the disc's centre the dot is looked for, as a share
    /// of the radius.
    pub reach: f64,
    /// Fewest pixels the dot has.
    pub min_pixels: usize,
    /// Largest width or height of the dot, as a share of the radius,
    pub max_size: f64,
    /// plus this many pixels: its antialiased edge falls on whole pixels.
    pub slack: f64,
}

/// The range rings drawn inside the disc, and the distances printed
/// beside them.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RingProfile {
    /// Points sampled round each circle.
    pub samples: usize,
    /// Least share of a circle's points that must be in view and clear
    /// of any window for its brightness to count.
    pub min_visible: f64,
    /// The radii searched for rings, as shares of the disc's radius:
    /// clear of the player's dot and of the rim's shadow.
    pub span: [f64; 2],
    /// A ring is where the middling brightness round the circle stands at
    /// least this far above that of the map as a whole.
    pub prominence: f64,
    /// The metres per pixel the labelled rings give must agree within
    /// this share.
    pub tolerance: f64,
    /// The unit printed after each ring's distance.
    pub unit: String,
    /// Fewest rings whose distance must be read: a scale needs two
    /// distances to check one against the other.
    pub min_labels: usize,
    /// The labels sit across the line from the centre to the disc's right
    /// edge, each over its ring; they are looked for within this share of
    /// the radius above and below that line.
    pub label_reach: f64,
    /// How the labels' strokes group into runs.
    pub lettering: TextProfile,
    /// How the labels are told from the map behind them: they are a pale
    /// green, so they are allowed more colour than lettering on the scene,
    /// and they stand further above the map than the paler ring lines.
    pub stroke: StrokeProfile,
}

/// Which edge of the disc a coordinate is aligned with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aligned {
    Left,
    Right,
}

/// One of the two coordinates.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinateProfile {
    /// The label printed before the value.
    pub label: String,
    /// The disc's edge the lettering is aligned with; it is looked for in
    /// that half of the band.
    pub aligned: Aligned,
    /// Where lettering worth reading has its aligned end, from that edge,
    /// `[least, most]`, in heights of the lettering, rightward positive.
    /// Only for choosing which lettering to read: scenery joined to the
    /// lettering can make it taller, so this is not what accepts it.
    pub offset: [f64; 2],
    /// Where the aligned end must be, in digit pitches measured by the
    /// label (see `label_pitches`): what accepts the lettering's place.
    pub anchor: [f64; 2],
    /// The ink of the label, from its first column to its last, in digit
    /// pitches: where the label is predicted to end.
    pub label_pitches: f64,
    /// The space between the label's ink and the first digit's, in digit
    /// pitches. The client prints the label and its space the same
    /// whatever the value, so the two together are the ruler the value's
    /// digits are measured with.
    pub space_pitches: f64,
    /// How wide the label and the space after it are, in digits, for
    /// comparing the two values' widths.
    pub label_digits: f64,
}

/// The coordinates printed under the disc.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatesProfile {
    pub lon: CoordinateProfile,
    pub lat: CoordinateProfile,
    /// How many digits a coordinate has, `[fewest, most]`.
    pub digits: [usize; 2],
    /// The band holding the coordinates, from and to, in shares of the
    /// disc's radius below the disc's lowest point.
    pub band: [f64; 2],
    /// How far the band reaches past the disc's sides, as a share of the
    /// radius.
    pub band_reach: f64,
    /// How the lettering's strokes group into runs.
    pub lettering: TextProfile,
    /// How the lettering is told from the scene behind it.
    pub stroke: StrokeProfile,
    /// Rows above and below the lettering that a window may not reach
    /// into: a window along the top or bottom cuts every glyph.
    pub guard_rows: i32,
    /// The label's end is looked for within this many digit pitches of
    /// where it is predicted,
    pub split_reach: f64,
    /// in a gap of empty columns at least this many digit pitches wide.
    pub min_space: f64,
    /// The digits' ink falls short of a whole number of pitches by this
    /// many: the side bearings of the first and last digit.
    pub digit_trim: f64,
    /// Largest difference, in digits, between the digits' ink and the
    /// width the reading predicts.
    pub ruler_tolerance: f64,
    /// Largest share by which the ratio of the two values' widths may
    /// differ from what their labels and digit counts predict.
    pub pair_tolerance: f64,
}

/// The radar found in a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Radar {
    /// The player's dot, the centre of the map and of its rings.
    pub x: f64,
    pub y: f64,
    /// The radius of the map disc.
    pub radius: f64,
}

impl Radar {
    /// Everything a reader of the radar looks at: the disc and its rim,
    /// the band under it where the coordinates are printed, and the band
    /// above it where the planet's name is, with the profile's margin.
    pub fn neighbourhood(&self, profile: &RadarProfile) -> Rect {
        let r = self.radius;
        let side = r
            * (1.0
                + profile
                    .coordinates
                    .band_reach
                    .max(profile.planet.reach - 1.0));
        let margin = profile.neighbourhood_margin * r;
        Rect::new(
            (self.x - side - margin).floor() as i32,
            (self.y - r - profile.planet.band[1] * r - margin).floor() as i32,
            (self.x + side + margin).ceil() as i32,
            (self.y + r + profile.coordinates.band[1] * r + margin).ceil() as i32,
        )
    }

    /// The disc's bounding rectangle.
    pub fn disc(&self) -> Rect {
        Rect::new(
            (self.x - self.radius).floor() as i32,
            (self.y - self.radius).floor() as i32,
            (self.x + self.radius).ceil() as i32,
            (self.y + self.radius).ceil() as i32,
        )
    }
}

/// A circle, `(x, y, radius)`.
type Circle = (f64, f64, f64);

/// Whether `rgb` is the map's green.
fn map_green(rgb: [u8; 3], disc: &DiscProfile) -> bool {
    let [r, g, b] = rgb;
    g >= disc.min_green
        && g.saturating_sub(r) >= disc.green_over_red
        && g.saturating_sub(b) >= disc.green_over_blue
}

/// Whether `rgb` is greener than it is red or blue by the edge margin:
/// the map, including its darkened border, but not the grey rim.
fn greenish(rgb: [u8; 3], disc: &DiscProfile) -> bool {
    let [r, g, b] = rgb;
    g.saturating_sub(r.max(b)) >= disc.edge_green_margin
}

/// Connected regions of map-green cells, each as the bounding rectangle
/// of its cells in pixels, largest first.
fn green_regions(image: &BgrImage, disc: &DiscProfile) -> Vec<Rect> {
    let c = disc.cell.max(1);
    let (cols, rows) = (image.width() / c, image.height() / c);
    if cols == 0 || rows == 0 {
        return Vec::new();
    }
    let need = (disc.cell_share * (c * c) as f32).ceil() as i32;
    let mut green = vec![false; (cols * rows) as usize];
    for row in 0..rows {
        for col in 0..cols {
            let mut count = 0;
            for y in row * c..(row + 1) * c {
                for x in col * c..(col + 1) * c {
                    if map_green(image.rgb(x, y), disc) {
                        count += 1;
                    }
                }
            }
            green[(row * cols + col) as usize] = count >= need;
        }
    }
    let mut seen = vec![false; green.len()];
    let mut regions: Vec<(usize, Rect)> = Vec::new();
    for start in 0..green.len() {
        if !green[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let mut cells = 0usize;
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        while let Some(i) = stack.pop() {
            cells += 1;
            let (col, row) = (i as i32 % cols, i as i32 / cols);
            x0 = x0.min(col);
            y0 = y0.min(row);
            x1 = x1.max(col);
            y1 = y1.max(row);
            for (dc, dr) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (nc, nr) = (col + dc, row + dr);
                if nc < 0 || nr < 0 || nc >= cols || nr >= rows {
                    continue;
                }
                let n = (nr * cols + nc) as usize;
                if green[n] && !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        regions.push((
            cells,
            Rect::new(x0 * c, y0 * c, (x1 + 1) * c - 1, (y1 + 1) * c - 1),
        ));
    }
    regions.sort_by_key(|(cells, _)| std::cmp::Reverse(*cells));
    regions.into_iter().map(|(_, rect)| rect).collect()
}

/// Where a ray from `(cx, cy)` at `angle` leaves the map for the rim,
/// searching radii `from..=to`: the boundary just before the first pixel
/// that is not greenish when the pixel before it is, and the following
/// run is the rim's (no green, mostly dark). `None` when the ray finds
/// no such edge or leaves the frame first.
fn edge_along(
    image: &BgrImage,
    (cx, cy): (f64, f64),
    angle: f64,
    (from, to): (i32, i32),
    disc: &DiscProfile,
) -> Option<(f64, f64)> {
    let (dx, dy) = (angle.cos(), angle.sin());
    let mut samples: Vec<(bool, bool)> = Vec::new();
    for r in from..=to + disc.edge_run {
        let (x, y) = (
            (cx + r as f64 * dx).round() as i32,
            (cy + r as f64 * dy).round() as i32,
        );
        if !image.in_bounds(x, y) {
            break;
        }
        let rgb = image.rgb(x, y);
        samples.push((greenish(rgb, disc), image.lum(x, y) <= disc.rim_max_lum));
    }
    let run = disc.edge_run.max(1) as usize;
    let need_dark = (disc.rim_share * run as f32).ceil() as usize;
    for i in 1..samples.len().saturating_sub(run - 1) {
        if !samples[i - 1].0 {
            continue;
        }
        let window = &samples[i..i + run];
        if window.iter().all(|s| !s.0) && window.iter().filter(|s| s.1).count() >= need_dark {
            let r = (from + i as i32) as f64 - 0.5;
            return Some((cx + r * dx, cy + r * dy));
        }
    }
    None
}

/// A determinant this near zero leaves a circle fit undefined: the
/// points lie in a line, or as near one as floating point can tell.
const SINGULAR: f64 = 1e-9; // not layout: floating-point tolerance of a circle fit

/// The circle through three points, if they are not in a line.
fn circumcircle(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> Option<Circle> {
    let d = 2.0 * (a.0 * (b.1 - c.1) + b.0 * (c.1 - a.1) + c.0 * (a.1 - b.1));
    if d.abs() < SINGULAR {
        return None;
    }
    let (a2, b2, c2) = (
        a.0 * a.0 + a.1 * a.1,
        b.0 * b.0 + b.1 * b.1,
        c.0 * c.0 + c.1 * c.1,
    );
    let x = (a2 * (b.1 - c.1) + b2 * (c.1 - a.1) + c2 * (a.1 - b.1)) / d;
    let y = (a2 * (c.0 - b.0) + b2 * (a.0 - c.0) + c2 * (b.0 - a.0)) / d;
    Some((x, y, ((a.0 - x).powi(2) + (a.1 - y).powi(2)).sqrt()))
}

/// The least-squares circle through `points` (the algebraic fit, taken
/// about the points' mean for stability).
fn least_squares_circle(points: &[(f64, f64)]) -> Option<Circle> {
    if points.len() < 3 {
        return None;
    }
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let (mut suu, mut suv, mut svv, mut suuu, mut svvv, mut suvv, mut svuu) =
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for &(x, y) in points {
        let (u, v) = (x - mx, y - my);
        suu += u * u;
        suv += u * v;
        svv += v * v;
        suuu += u * u * u;
        svvv += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let det = suu * svv - suv * suv;
    if det.abs() < SINGULAR {
        return None;
    }
    let (bu, bv) = ((suuu + suvv) / 2.0, (svvv + svuu) / 2.0);
    let uc = (bu * svv - bv * suv) / det;
    let vc = (bv * suu - bu * suv) / det;
    let r = (uc * uc + vc * vc + (suu + svv) / n).sqrt();
    Some((uc + mx, vc + my, r))
}

/// The points within the profile's tolerance of `circle`.
fn on_circle(points: &[(f64, f64)], circle: Circle, disc: &DiscProfile) -> Vec<(f64, f64)> {
    let tolerance = disc.fit_tolerance * circle.2;
    points
        .iter()
        .copied()
        .filter(|p| {
            (((p.0 - circle.0).powi(2) + (p.1 - circle.1).powi(2)).sqrt() - circle.2).abs()
                <= tolerance
        })
        .collect()
}

/// The circle most of `points` lie on, with a radius in `radii`. Some
/// edges end early, on a button set into the rim or on a window in
/// front, so the circle is the one through three of the points that the
/// most others agree with, then refined by least squares over those that
/// agree. The radius is bounded because three points along a window's
/// straight edge make a vast circle that every point is near.
fn robust_circle(
    points: &[(f64, f64)],
    radii: (f64, f64),
    disc: &DiscProfile,
) -> Option<(Circle, Vec<(f64, f64)>)> {
    let n = points.len();
    if n < 3 {
        return None;
    }
    let plausible =
        |c: &Circle| c.0.is_finite() && c.1.is_finite() && c.2 >= radii.0 && c.2 <= radii.1;
    let mut best: Option<(Circle, usize)> = None;
    for step in disc
        .fit_divisions
        .iter()
        .filter(|d| **d > 0)
        .map(|d| n / d)
        .filter(|s| *s > 0)
    {
        for i in 0..n {
            let Some(circle) = circumcircle(
                points[i],
                points[(i + step) % n],
                points[(i + 2 * step) % n],
            )
            .filter(plausible) else {
                continue;
            };
            let count = on_circle(points, circle, disc).len();
            if best.is_none_or(|(_, c)| count > c) {
                best = Some((circle, count));
            }
        }
    }
    let (mut circle, _) = best?;
    for _ in 0..disc.fit_passes {
        let agreeing = on_circle(points, circle, disc);
        circle = least_squares_circle(&agreeing).filter(plausible)?;
    }
    let agreeing = on_circle(points, circle, disc);
    Some((circle, agreeing))
}

/// The edge of the disc seen from `(cx, cy)`, as a circle, when it is a
/// circle seen most of the way round.
fn fit_disc(
    image: &BgrImage,
    (cx, cy): (f64, f64),
    radius: f64,
    disc: &DiscProfile,
) -> Option<Circle> {
    let rays = disc.rays.max(3);
    let span = (
        (radius * disc.ray_span[0]) as i32,
        (radius * disc.ray_span[1]).ceil() as i32,
    );
    let points: Vec<(f64, f64)> = (0..rays)
        .filter_map(|k| {
            let angle = std::f64::consts::TAU * k as f64 / rays as f64;
            edge_along(image, (cx, cy), angle, span, disc)
        })
        .collect();
    let radii = (
        (span.0 as f64).max(disc.radius[0]),
        (span.1 as f64).min(disc.radius[1]),
    );
    let (circle, agreeing) = robust_circle(&points, radii, disc)?;
    if agreeing.len() < 3 || (agreeing.len() as f64) < disc.min_on_circle * rays as f64 {
        return None;
    }
    // The points on the circle must surround it, not bunch on one arc.
    let mut angles: Vec<f64> = agreeing
        .iter()
        .map(|p| (p.1 - circle.1).atan2(p.0 - circle.0).to_degrees())
        .collect();
    angles.sort_by(f64::total_cmp);
    let widest = angles
        .windows(2)
        .map(|w| w[1] - w[0])
        .chain(std::iter::once(
            angles[0] + TURN_DEGREES - angles[angles.len() - 1],
        ))
        .fold(0.0, f64::max);
    (widest <= disc.max_arc_gap).then_some(circle)
}

/// The player's dot near `(cx, cy)`: its centre.
fn find_dot(
    image: &BgrImage,
    (cx, cy): (f64, f64),
    radius: f64,
    dot: &DotProfile,
) -> Option<(f64, f64)> {
    if !(cx.is_finite() && cy.is_finite() && radius.is_finite()) {
        return None;
    }
    let reach = (dot.reach * radius).ceil() as i32;
    let (x0, y0) = (cx.round() as i32 - reach, cy.round() as i32 - reach);
    let mut pixels: Vec<(i32, i32)> = Vec::new();
    for y in y0..=y0 + 2 * reach {
        for x in x0..=x0 + 2 * reach {
            if !image.in_bounds(x, y) {
                continue;
            }
            let rgb = image.rgb(x, y);
            if rgb.iter().all(|c| *c >= dot.min_channel) && chroma(rgb) <= dot.max_chroma {
                pixels.push((x, y));
            }
        }
    }
    if pixels.is_empty() || pixels.len() < dot.min_pixels {
        return None;
    }
    let span = |of: fn(&(i32, i32)) -> i32| {
        let lo = pixels.iter().map(of).min().expect("not empty");
        let hi = pixels.iter().map(of).max().expect("not empty");
        (hi - lo + 1) as f64
    };
    let largest = dot.max_size * radius + dot.slack;
    if span(|p| p.0) > largest || span(|p| p.1) > largest {
        return None;
    }
    let n = pixels.len() as f64;
    Some((
        pixels.iter().map(|p| p.0 as f64).sum::<f64>() / n,
        pixels.iter().map(|p| p.1 as f64).sum::<f64>() / n,
    ))
}

/// The radar in `image`, if one is in view: the player's dot at its
/// centre and the radius of its map disc. `None` when no green region is
/// a rimmed disc with the dot at its centre, which is the case when a
/// window covers the radar or the player has hidden it.
pub fn find_radar(image: &BgrImage, profile: &UiProfile) -> Option<Radar> {
    let disc = &profile.radar.disc;
    for region in green_regions(image, disc) {
        let size = region.width().max(region.height()) as f64;
        if size < 2.0 * disc.radius[0] {
            continue;
        }
        let start = (
            (region.x0 + region.x1) as f64 / 2.0,
            (region.y0 + region.y1) as f64 / 2.0,
        );
        // A window over part of the disc leaves its region short of the
        // disc, so the first fit's centre is a step closer, and the
        // second is cast from where the disc is.
        let Some(first) = fit_disc(image, start, size / 2.0, disc) else {
            continue;
        };
        let Some(circle) = fit_disc(image, (first.0, first.1), first.2, disc) else {
            continue;
        };
        if let Some((x, y)) = find_dot(image, (circle.0, circle.1), circle.2, &disc.dot) {
            return Some(Radar {
                x,
                y,
                radius: circle.2,
            });
        }
    }
    None
}

/// The map's scale, from its range rings and the distances printed on
/// them.
#[derive(Debug, Clone, PartialEq)]
pub struct RingScale {
    /// The rings whose distance was read, `(radius in pixels, distance in
    /// metres)`, nearest first.
    pub rings: Vec<(f64, f64)>,
    /// Rings found whose label could not be read (a compass letter can
    /// stand over one), by radius.
    pub unlabelled: Vec<f64>,
    /// The labels as the recogniser read them.
    pub labels: Vec<String>,
    pub metres_per_pixel: f64,
    /// How far the disc's edge is from the player, in metres.
    pub edge_range_m: f64,
}

/// The middle value of `values`, which it reorders.
fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[values.len() / 2])
}

/// The fewest points round the circle the ring search samples, whatever
/// a profile says: fewer cannot tell a ring from a stray line.
const MIN_RING_SAMPLES: usize = 8; // not layout: a floor on what a profile may ask for

/// The radii of the rings in `radar`: each a stretch of radii whose
/// middling brightness round the circle stands out from the map's,
/// placed at the centre of its brightness. `None` when a stretch runs to
/// either end of the searched span, whose ring may lie partly beyond it,
/// or when too little of a circle is in view.
fn ring_radii(
    image: &BgrImage,
    radar: &Radar,
    windows: &[Rect],
    rings: &RingProfile,
) -> Option<Vec<f64>> {
    let samples = rings.samples.max(MIN_RING_SAMPLES);
    let from = (rings.span[0] * radar.radius).ceil() as i32;
    let to = (rings.span[1] * radar.radius).floor() as i32;
    if to <= from {
        return None;
    }
    // The rings are pale on every side, while the map's lines, mobs and
    // lettering cross only a few of the circle's points; a window in
    // front hides some, and they are left out.
    let mut by_radius: Vec<(i32, f64)> = Vec::new();
    for r in from..=to {
        let mut values: Vec<f64> = (0..samples)
            .filter_map(|k| {
                let angle = std::f64::consts::TAU * k as f64 / samples as f64;
                let x = (radar.x + r as f64 * angle.cos()).round() as i32;
                let y = (radar.y + r as f64 * angle.sin()).round() as i32;
                (image.in_bounds(x, y) && !windows.iter().any(|w| w.contains(x, y)))
                    .then(|| image.lum(x, y) as f64)
            })
            .collect();
        if (values.len() as f64) < rings.min_visible * samples as f64 {
            return None;
        }
        by_radius.push((r, median(&mut values)?));
    }
    let mut levels: Vec<f64> = by_radius.iter().map(|(_, v)| *v).collect();
    let base = median(&mut levels)?;
    let stands_out = |i: usize| by_radius[i].1 - base >= rings.prominence;
    if stands_out(0) || stands_out(by_radius.len() - 1) {
        return None;
    }
    let mut found = Vec::new();
    let mut i = 0;
    while i < by_radius.len() {
        if !stands_out(i) {
            i += 1;
            continue;
        }
        let start = i;
        while i < by_radius.len() && stands_out(i) {
            i += 1;
        }
        // The stretch with its soft shoulders, one radius either side.
        let (mut weight, mut moment) = (0.0, 0.0);
        for &(r, level) in &by_radius[start - 1..=i] {
            let e = (level - base).max(0.0);
            weight += e;
            moment += e * r as f64;
        }
        if weight <= 0.0 {
            return None;
        }
        let radius = moment / weight;
        if !radius.is_finite() {
            return None;
        }
        found.push(radius);
    }
    Some(found)
}

/// The scale of the map in `radar`, from the rings drawn on it and the
/// distances printed beside them. The client can zoom the radar, which
/// changes the distances the same rings stand for, so the distances are
/// read, never assumed: each label is read over its ring, must be a
/// distance in the profile's unit, and the distances must grow with the
/// rings in the proportion of their radii. A ring whose label cannot be
/// read is left out; at least the profile's fewest must be read. `None`
/// when they are not (the client can hide the distances), or when the
/// rings and the distances disagree. A scale is never guessed.
pub fn ring_scale(
    image: &BgrImage,
    radar: &Radar,
    profile: &UiProfile,
    recogniser: &dyn Recogniser,
) -> Option<RingScale> {
    let rings = &profile.radar.rings;
    let windows: Vec<Rect> = find_windows(image, &profile.chrome)
        .into_iter()
        .map(|w| w.rect)
        .collect();
    let radii = ring_radii(image, radar, &windows, rings)?;
    let reach = rings.label_reach * radar.radius;
    let strip = Rect::new(
        (radar.x + rings.span[0] * radar.radius).floor() as i32,
        (radar.y - reach).floor() as i32,
        (radar.x + radar.radius).floor() as i32,
        (radar.y + reach).ceil() as i32,
    );
    let mut labelled: Vec<(f64, f64)> = Vec::new();
    let mut labels = Vec::new();
    for run in scenery_runs(image, strip, &rings.lettering, &rings.stroke, &windows) {
        let on_axis = (run.rect.y0 as f64) <= radar.y && (run.rect.y1 as f64) >= radar.y;
        let crossed: Vec<f64> = radii
            .iter()
            .copied()
            .filter(|r| {
                let x = radar.x + r;
                x >= run.rect.x0 as f64 && x <= run.rect.x1 as f64
            })
            .collect();
        if !on_axis || crossed.is_empty() {
            continue;
        }
        let (raw, _) = read_run(image, run.rect, &profile.read, recogniser)?;
        // A label over two touching rings reads as both distances; each
        // ring under the label takes the distance in its place.
        if let Some(distances) = distance_labels(&raw, &rings.unit) {
            if distances.len() == crossed.len() {
                labelled.extend(crossed.iter().zip(&distances).map(|(r, d)| (*r, *d as f64)));
            }
        }
        labels.push(raw);
    }
    labelled.sort_by(|a, b| a.0.total_cmp(&b.0));
    if labelled.len() < rings.min_labels.max(2) || labelled.windows(2).any(|w| w[1].1 <= w[0].1) {
        return None;
    }
    let metres_per_pixel = labelled.iter().map(|(r, d)| r * d).sum::<f64>()
        / labelled.iter().map(|(r, _)| r * r).sum::<f64>();
    if !metres_per_pixel.is_finite() || metres_per_pixel <= 0.0 {
        return None;
    }
    // Written so that a value that is not a number fails it.
    let agrees = |(r, d): &(f64, f64)| (d / r / metres_per_pixel - 1.0).abs() <= rings.tolerance;
    if !labelled.iter().all(agrees) {
        return None;
    }
    let unlabelled = radii
        .iter()
        .copied()
        .filter(|r| !labelled.iter().any(|(l, _)| l == r))
        .collect();
    Some(RingScale {
        rings: labelled,
        unlabelled,
        labels,
        metres_per_pixel,
        edge_range_m: radar.radius * metres_per_pixel,
    })
}

/// Whether `radar` shows its range rings: rings stand out round its
/// centre, whether or not their distances are printed or read. A scale
/// that did not read from rings that are there means their labels did
/// not read (the client can hide them); no rings at all is another
/// matter.
pub fn rings_found(image: &BgrImage, radar: &Radar, profile: &UiProfile) -> bool {
    let windows: Vec<Rect> = find_windows(image, &profile.chrome)
        .into_iter()
        .map(|w| w.rect)
        .collect();
    ring_radii(image, radar, &windows, &profile.radar.rings).is_some_and(|radii| !radii.is_empty())
}

/// The planet's name printed above `radar`, when it reads as exactly one
/// of `names` (compared without regard to case or repeated spaces); the
/// name is answered as `names` spells it. `None` when the name is not in
/// view, is covered or cut, or reads as anything else: a name that did
/// not read says nothing about where the player is.
///
/// The name is centred above the disc, the one piece of lettering there
/// near the disc's middle; it must lie wholly inside its band, clear of
/// any window.
pub fn read_planet_label(
    image: &BgrImage,
    radar: &Radar,
    profile: &UiProfile,
    names: &[&str],
    recogniser: &dyn Recogniser,
) -> Option<String> {
    let label = &profile.radar.planet;
    let r = radar.radius;
    let top = radar.y - r;
    let band = Rect::new(
        (radar.x - label.reach * r).floor() as i32,
        (top - label.band[1] * r).floor() as i32,
        (radar.x + label.reach * r).ceil() as i32,
        (top - label.band[0] * r).ceil() as i32,
    )
    .intersect(&Rect::new(0, 0, image.width() - 1, image.height() - 1))?;
    let windows: Vec<Rect> = find_windows(image, &profile.chrome)
        .into_iter()
        .map(|w| w.rect)
        .collect();
    let runs = scenery_runs(image, band, &label.lettering, &label.stroke, &windows);
    let mut centred = runs.iter().filter(|run| {
        let middle = (run.rect.x0 + run.rect.x1) as f64 / 2.0;
        (middle - radar.x).abs() <= label.centred * r
    });
    let run = centred.next()?;
    if centred.next().is_some() {
        return None;
    }
    let rect = run.rect;
    // Lettering at the band's edge may be cut, and a cut name can read as
    // a shorter name that is also a planet's.
    if rect.x0 <= band.x0 || rect.y0 <= band.y0 || rect.x1 >= band.x1 || rect.y1 >= band.y1 {
        return None;
    }
    let guard = rect.padded(label.guard_rows);
    if windows.iter().any(|w| w.intersect(&guard).is_some()) {
        return None;
    }
    let (raw, _) = read_run(image, rect, &profile.read, recogniser)?;
    let spoken = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let read = spoken(&raw);
    names
        .iter()
        .find(|name| spoken(name) == read)
        .map(|name| name.to_string())
}

/// The range of coordinates a value may have: those of the planet the
/// player is on, which the caller knows from the map selected. Each pair
/// is `(least, most)`, inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoordinateBounds {
    pub lon: (i64, i64),
    pub lat: (i64, i64),
}

/// Why a coordinate, or the radar itself, was not read. Every variant is
/// a refusal.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CoordError {
    #[error("no recogniser is available")]
    NoRecogniser,
    #[error("no radar is in view: it is hidden, covered by a window, or off the screen")]
    RadarNotFound,
    #[error("the band under the radar where the value is printed runs off the screen")]
    OffScreen,
    #[error("a window lies over the band where the value is printed")]
    Covered,
    #[error("no lettering stands where the value is printed")]
    NotFound,
    #[error("more than one piece of lettering stands where the value is printed")]
    Ambiguous,
    #[error("the lettering runs out of the band where the value is printed, so it may be cut")]
    Clipped,
    #[error("the value read as {raw:?}, which is not its label followed by a coordinate")]
    Invalid { raw: String },
    #[error("the value read as {raw:?}, but its label and its digits cannot be told apart")]
    NoSpace { raw: String },
    #[error("the value read as {raw:?}, but its lettering does not end at the disc's edge")]
    Misplaced { raw: String },
    #[error("the value read as {raw:?}, but its digits are not as wide as that many digits")]
    WrongWidth { raw: String },
    #[error("the value read as {text:?}, which is outside the planet's coordinates")]
    OutOfBounds { text: String },
    #[error("the values read as {lon:?} and {lat:?}, but their lettering is not as wide as that")]
    WidthsDisagree { lon: String, lat: String },
}

/// One coordinate read and validated.
#[derive(Debug, Clone, PartialEq)]
pub struct Coordinate {
    pub value: u32,
    /// The value with its label, as the client prints it: `LON 67739`.
    pub text: String,
    /// The recogniser's text, before validation.
    pub raw: String,
    /// The lettering's bounds.
    pub rect: Rect,
}

/// The coordinates under a radar, each read or refused on its own: a
/// window can cover one and not the other.
#[derive(Debug, Clone, PartialEq)]
pub struct Coordinates {
    pub radar: Radar,
    pub lon: Result<Coordinate, CoordError>,
    pub lat: Result<Coordinate, CoordError>,
}

/// The half of the band under the radar where `value` is printed.
///
/// The lettering's size does not follow the radar's: `LON 67739` is
/// about 75 pixels wide under both the default and the enlarged radar.
/// Its distance below the disc does grow with the radius (about 29, 37
/// and 51 pixels at radii of 113, 140 and 169), so the band is placed in
/// shares of the radius, and its extent, a third of the radius tall and
/// reaching from past the disc's side to its centre, holds lettering of
/// that size for every radar whose radius is at least about 70 pixels.
/// Under a smaller radar the lettering reaches the band's edge and is
/// refused as cut, never read short.
fn value_band(radar: &Radar, value: &CoordinateProfile, coords: &CoordinatesProfile) -> Rect {
    let r = radar.radius;
    let bottom = radar.y + r;
    let (y0, y1) = (
        (bottom + coords.band[0] * r).floor() as i32,
        (bottom + coords.band[1] * r).ceil() as i32,
    );
    match value.aligned {
        Aligned::Left => Rect::new(
            (radar.x - r - coords.band_reach * r).floor() as i32,
            y0,
            radar.x.floor() as i32,
            y1,
        ),
        Aligned::Right => Rect::new(
            radar.x.floor() as i32 + 1,
            y0,
            (radar.x + r + coords.band_reach * r).ceil() as i32,
            y1,
        ),
    }
}

/// Where `value`'s aligned end stands from its edge of the disc, in
/// pixels, rightward positive.
fn end_offset(rect: &Rect, radar: &Radar, value: &CoordinateProfile) -> f64 {
    match value.aligned {
        Aligned::Left => rect.x0 as f64 - (radar.x - radar.radius),
        Aligned::Right => rect.x1 as f64 - (radar.x + radar.radius),
    }
}

/// Whether `run` is where lettering worth reading would be: a first,
/// loose choice among the runs in the band (see `CoordinateProfile`).
fn worth_reading(run: &TextRun, radar: &Radar, value: &CoordinateProfile) -> bool {
    let offset = end_offset(&run.rect, radar, value) / run.rect.height() as f64;
    offset >= value.offset[0] && offset <= value.offset[1]
}

/// The lettering's columns that hold ink, left to right.
fn ink_columns(
    image: &BgrImage,
    rect: Rect,
    windows: &[Rect],
    coords: &CoordinatesProfile,
) -> Vec<bool> {
    (rect.x0..=rect.x1)
        .map(|x| {
            (rect.y0..=rect.y1).any(|y| {
                !windows.iter().any(|w| w.contains(x, y))
                    && stroke_ink(image, x, y, coords.lettering.min_ink, &coords.stroke)
            })
        })
        .collect()
}

/// The label's measure of the value's lettering: the digit pitch, and
/// the digits' ink in pitches.
struct Measure {
    pitch: f64,
    digits: f64,
}

/// Measure `columns` (the lettering's ink, column by column) for a value
/// read with `digits` digits, by its label. Where the label ends is
/// predicted from the reading: the whole lettering is the label, the
/// space and the digits, so it gives a pitch, and the label ends that
/// many pitches in. The end is taken at the gap of empty columns found
/// there, and `None` (the split is not clear) when there is none. The
/// label and its space, from the label's first ink to the first digit's,
/// then give the pitch the digits are measured in, so a reading with the
/// wrong number of digits moves the prediction a little but leaves the
/// measure to the ink. The label's own last column is not used: the
/// outer stroke of its last letter is the faintest, and against a pale
/// scene it can be lost, while a digit's first stroke is a full upright.
fn measure(
    columns: &[bool],
    digits: usize,
    value: &CoordinateProfile,
    coords: &CoordinatesProfile,
) -> Option<Measure> {
    let width = columns.len() as f64;
    let predicted =
        width / (value.label_pitches + value.space_pitches + digits as f64 - coords.digit_trim);
    let label_end = value.label_pitches * predicted;
    let reach = coords.split_reach * predicted;
    let narrowest = coords.min_space * predicted;
    let mut gaps: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < columns.len() {
        if columns[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < columns.len() && !columns[i] {
            i += 1;
        }
        gaps.push((start, i));
    }
    // Distance from the predicted end to a gap: none when it falls inside.
    let distance = |&(start, end): &(usize, usize)| {
        if label_end < start as f64 {
            start as f64 - label_end
        } else if label_end > end as f64 {
            label_end - end as f64
        } else {
            0.0
        }
    };
    let (_, end) = gaps
        .iter()
        .filter(|&&(start, end)| start > 0 && end < columns.len())
        .filter(|&&(start, end)| (end - start) as f64 >= narrowest)
        .filter(|gap| distance(gap) <= reach)
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        .copied()?;
    let pitch = end as f64 / (value.label_pitches + value.space_pitches);
    let measured = (columns.len() - end) as f64 / pitch;
    (pitch.is_finite() && pitch > 0.0 && measured.is_finite()).then_some(Measure {
        pitch,
        digits: measured,
    })
}

/// Read one coordinate. `Ok(None)` is no recogniser.
fn read_value(
    image: &BgrImage,
    radar: &Radar,
    windows: &[Rect],
    value: &CoordinateProfile,
    bounds: (i64, i64),
    profile: &UiProfile,
    recogniser: &dyn Recogniser,
) -> Result<Option<Coordinate>, CoordError> {
    let coords = &profile.radar.coordinates;
    let band = value_band(radar, value, coords);
    if band.x0 < 0 || band.y0 < 0 || band.x1 >= image.width() || band.y1 >= image.height() {
        return Err(CoordError::OffScreen);
    }
    // A window over the band can hide the value, or put lettering of its
    // own where the value should be; either way that is why nothing
    // usable is found there.
    let over_band = windows.iter().any(|w| w.intersect(&band).is_some());
    let missing = |refusal: CoordError| {
        if over_band {
            CoordError::Covered
        } else {
            refusal
        }
    };
    let runs = scenery_runs(image, band, &coords.lettering, &coords.stroke, windows);
    let mut candidates = runs.iter().filter(|run| worth_reading(run, radar, value));
    let run = candidates
        .next()
        .ok_or_else(|| missing(CoordError::NotFound))?;
    if candidates.next().is_some() {
        return Err(missing(CoordError::Ambiguous));
    }
    // Lettering that reaches the band's edge may carry on past it, and a
    // digit cut off there would leave a shorter number that still reads.
    let r = run.rect;
    if r.x0 <= band.x0 || r.y0 <= band.y0 || r.x1 >= band.x1 || r.y1 >= band.y1 {
        return Err(CoordError::Clipped);
    }
    // The same goes for a window in front. The digits come last, so a
    // window beside the lettering's right end, anywhere within joining
    // distance, may hide or cut a digit, and one along its top or bottom
    // may cut every glyph; either leaves a number that can still read.
    // A window at its left end can only hide part of the label, which
    // must then still read exactly as the label, so it is not refused.
    let reach = (coords.lettering.join_factor * r.height() as f32).ceil() as i32;
    let guard = Rect::new(
        r.x0,
        r.y0 - coords.guard_rows,
        r.x1 + reach,
        r.y1 + coords.guard_rows,
    );
    if windows.iter().any(|w| w.intersect(&guard).is_some()) {
        return Err(CoordError::Covered);
    }
    let Some((raw, _)) = read_run(image, r, &profile.read, recogniser) else {
        return Ok(None);
    };
    let invalid = || CoordError::Invalid { raw: raw.clone() };
    let parsed = labelled_integer(&raw, &value.label, coords.digits).ok_or_else(invalid)?;
    let number = parsed
        .number
        .and_then(|n| u32::try_from(n as u64).ok().filter(|v| *v as f64 == n))
        .ok_or_else(invalid)?;
    let digits = parsed.text.chars().filter(|c| c.is_ascii_digit()).count();
    // The client's numeral widths (`validate::width_agrees`) are measured
    // on the sans-serif lettering inside windows; these are serif, and
    // drawn on the scene. They are measured against their own label
    // instead, which needs no metric but the label's width in pitches.
    //
    // The error budget, measured on captures at four radar sizes and the
    // desktop-scaled one: the first digit's start is found to within a
    // column of about 37 from the label's start (the pitch to about 3%),
    // and the digits' ink to within a column of about 39, so the digits
    // measure within about a quarter of a digit of what a correct reading
    // predicts. The tolerance sits between that and the whole digit that
    // a reading one short or one long is off by.
    let columns = ink_columns(image, r, windows, coords);
    let measured = measure(&columns, digits, value, coords)
        .ok_or_else(|| CoordError::NoSpace { raw: raw.clone() })?;
    let anchor = end_offset(&r, radar, value) / measured.pitch;
    if anchor < value.anchor[0] || anchor > value.anchor[1] {
        return Err(CoordError::Misplaced { raw });
    }
    if (measured.digits - (digits as f64 - coords.digit_trim)).abs() > coords.ruler_tolerance {
        return Err(CoordError::WrongWidth { raw });
    }
    if (number as i64) < bounds.0 || (number as i64) > bounds.1 {
        return Err(CoordError::OutOfBounds { text: parsed.text });
    }
    Ok(Some(Coordinate {
        value: number,
        text: parsed.text,
        raw,
        rect: r,
    }))
}

/// The number of digits in a coordinate's canonical text.
fn digit_count(coordinate: &Coordinate) -> f64 {
    coordinate
        .text
        .chars()
        .filter(|c| c.is_ascii_digit())
        .count() as f64
}

/// Whether the two values' widths agree with what they were read as, an
/// extra check when both read. The client prints both in one font, so the
/// ratio of their widths is fixed by their labels and digit counts.
fn widths_agree(lon: &Coordinate, lat: &Coordinate, coords: &CoordinatesProfile) -> bool {
    let predicted =
        (coords.lon.label_digits + digit_count(lon)) / (coords.lat.label_digits + digit_count(lat));
    let measured = lon.rect.width() as f64 / lat.rect.width() as f64;
    (measured / predicted - 1.0).abs() <= coords.pair_tolerance
}

/// Read the coordinates printed under the radar in `image`, each within
/// `bounds`, the coordinates of the planet the player is on.
///
/// `Err` when there is no radar to read under, or no recogniser; each
/// value is otherwise read or refused on its own.
pub fn read_coordinates(
    image: &BgrImage,
    profile: &UiProfile,
    bounds: &CoordinateBounds,
    recogniser: &dyn Recogniser,
) -> Result<Coordinates, CoordError> {
    let radar = find_radar(image, profile).ok_or(CoordError::RadarNotFound)?;
    let windows: Vec<Rect> = find_windows(image, &profile.chrome)
        .into_iter()
        .map(|w| w.rect)
        .collect();
    let coords = &profile.radar.coordinates;
    let read = |value: &CoordinateProfile,
                range: (i64, i64)|
     -> Result<Result<Coordinate, CoordError>, CoordError> {
        match read_value(image, &radar, &windows, value, range, profile, recogniser) {
            Ok(Some(coordinate)) => Ok(Ok(coordinate)),
            Ok(None) => Err(CoordError::NoRecogniser),
            Err(refusal) => Ok(Err(refusal)),
        }
    };
    let mut lon = read(&coords.lon, bounds.lon)?;
    let mut lat = read(&coords.lat, bounds.lat)?;
    if let (Ok(l), Ok(t)) = (&lon, &lat) {
        if !widths_agree(l, t, coords) {
            let refusal = CoordError::WidthsDisagree {
                lon: l.text.clone(),
                lat: t.text.clone(),
            };
            lon = Err(refusal.clone());
            lat = Err(refusal);
        }
    }
    Ok(Coordinates { radar, lon, lat })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::test_support::{
        blank, bundled_profile, draw_radar, draw_window, fill, put, scenery, within, Printed,
        RadarScene, DOT, HEIGHT, MAP, PITCH, RIM, RINGS, RING_LABELS, SHADOW,
    };

    fn assert_found(found: Option<Radar>, centre: (i32, i32), radius: i32) {
        let found = found.unwrap_or_else(|| panic!("no radar found at {centre:?} radius {radius}"));
        let s = (radius / 40).max(1) as f64;
        let (dot_x, dot_y) = (
            (centre.0 - (s as i32) / 2) as f64 + s / 2.0,
            (centre.1 - (s as i32) / 2) as f64 + s / 2.0,
        );
        assert!(
            (found.x - dot_x).abs() <= 1.0 && (found.y - dot_y).abs() <= 1.0,
            "{found:?}: the dot is at ({dot_x}, {dot_y})"
        );
        assert!(
            (found.radius - radius as f64).abs() <= (0.01 * radius as f64).max(1.0),
            "{found:?}: the radius is {radius}"
        );
    }

    #[test]
    fn a_radar_is_found_at_any_size_and_position() {
        let profile = bundled_profile();
        for (radius, centre) in [
            (45, (100, 500)),
            (97, (1100, 130)),
            (113, (1050, 160)),
            (169, (640, 360)),
            (260, (300, 330)),
        ] {
            let mut image = scenery(1280, 720, radius as u64);
            let rings: Vec<f64> = RINGS.iter().map(|r| r * radius as f64 / 113.0).collect();
            draw_radar(&mut image, centre, radius, &rings, true);
            assert_found(find_radar(&image, &profile), centre, radius);
        }
    }

    #[test]
    fn green_that_is_not_the_radar_is_ignored() {
        let profile = bundled_profile();
        let mut image = scenery(1280, 720, 7);
        // A green progress bar, a field of green with a dark border, a
        // green disc on the scene with no rim, and a rimmed green ellipse.
        fill(&mut image, Rect::new(40, 40, 600, 64), MAP);
        fill(&mut image, Rect::new(60, 120, 400, 400), RIM);
        fill(&mut image, Rect::new(72, 132, 388, 388), MAP);
        for y in 420..720 {
            for x in 450..800 {
                if within(x, y, (620.0, 570.0), 130.0) {
                    put(&mut image, x, y, MAP);
                }
            }
        }
        for y in 220..520 {
            for x in 840..1270 {
                let e = ((x as f64 + 0.5 - 1050.0) / 190.0).powi(2)
                    + ((y as f64 + 0.5 - 370.0) / 125.0).powi(2);
                if e < 1.0 {
                    put(&mut image, x, y, MAP);
                } else if e < 1.25 {
                    put(&mut image, x, y, RIM);
                }
            }
        }
        // Each green shape is given a white dot at its middle, so only
        // its shape and rim can tell it from the radar.
        for (x, y) in [(230, 260), (620, 570), (1050, 370), (320, 52)] {
            fill(&mut image, Rect::new(x - 1, y - 1, x + 1, y + 1), DOT);
        }
        // The skills window's green bars.
        let window = Rect::new(100, 450, 420, 700);
        draw_window(&mut image, window);
        for k in 0..4 {
            fill(
                &mut image,
                Rect::new(130, 500 + k * 40, 390, 512 + k * 40),
                [70, 200, 60],
            );
        }
        assert_eq!(find_radar(&image, &profile), None);

        // With the radar on screen too, the radar is what is found.
        let (centre, radius) = ((1120, 120), 90);
        draw_radar(
            &mut image,
            centre,
            radius,
            &RINGS.map(|r| r * 90.0 / 113.0),
            true,
        );
        assert_found(find_radar(&image, &profile), centre, radius);
    }

    #[test]
    fn a_rimmed_green_disc_without_the_players_dot_is_not_the_radar() {
        let profile = bundled_profile();
        let mut image = scenery(800, 600, 11);
        draw_radar(&mut image, (400, 300), 113, &RINGS, false);
        assert_eq!(find_radar(&image, &profile), None);
    }

    #[test]
    fn a_radar_under_a_maximised_window_is_not_found() {
        let profile = bundled_profile();
        let mut image = scenery(800, 600, 12);
        draw_radar(&mut image, (600, 160), 113, &RINGS, true);
        draw_window(&mut image, Rect::new(0, 0, 799, 599));
        assert_eq!(find_radar(&image, &profile), None);
    }

    #[test]
    fn a_radar_partly_under_a_window_is_still_measured_exactly() {
        let profile = bundled_profile();
        for (radius, cover) in [(113, 70), (169, 90)] {
            let mut image = scenery(1000, 700, 13);
            let centre = (700, 300);
            draw_radar(&mut image, centre, radius, &RINGS, true);
            // A window whose right edge lies over the disc's left side.
            draw_window(
                &mut image,
                Rect::new(200, 40, centre.0 - radius + cover, 650),
            );
            assert_found(find_radar(&image, &profile), centre, radius);
        }
    }

    #[test]
    fn a_search_with_no_white_pixel_finds_no_dot() {
        let image = blank(200, 200, MAP);
        let mut dot = bundled_profile().radar.disc.dot;
        assert_eq!(find_dot(&image, (100.0, 100.0), 80.0, &dot), None);
        // Even a profile that asks for no pixels at all finds none.
        dot.min_pixels = 0;
        assert_eq!(find_dot(&image, (100.0, 100.0), 80.0, &dot), None);
        assert_eq!(find_dot(&image, (f64::NAN, 100.0), 80.0, &dot), None);
    }

    // Ring scale.

    fn scale_of(radius: i32, rings: &[f64], labels: &[&str]) -> Option<RingScale> {
        let profile = bundled_profile();
        let mut scene = RadarScene::new(radius as u64 + 50);
        let centre = (640, 380);
        draw_radar(&mut scene.image, centre, radius, rings, true);
        for (ring, label) in rings.iter().zip(labels) {
            scene.ring_label(centre, *ring, label);
        }
        let radar = find_radar(&scene.image, &profile).expect("the radar is found");
        ring_scale(&scene.image, &radar, &profile, &scene.recogniser())
    }

    const LABELS: [&str; 3] = RING_LABELS;

    #[test]
    fn the_rings_and_their_distances_give_one_scale_whatever_the_radars_size() {
        // The client keeps the map's scale when the radar is resized, so
        // the rings sit at the same radii in a small and a large radar;
        // at another interface scale every radius is scaled alike.
        for (radius, factor) in [(113, 1.0), (169, 1.0), (250, 1.5)] {
            let rings: Vec<f64> = RINGS.iter().map(|r| r * factor).collect();
            let scale = scale_of(radius, &rings, &LABELS)
                .unwrap_or_else(|| panic!("radius {radius}: no scale"));
            let want = 200.0 / (103.0 * factor);
            assert!(
                (scale.metres_per_pixel / want - 1.0).abs() < 0.01,
                "radius {radius}: {scale:?}"
            );
            assert!(
                (scale.edge_range_m / (radius as f64 * want) - 1.0).abs() < 0.02,
                "radius {radius}: {scale:?}"
            );
            let metres: Vec<f64> = scale.rings.iter().map(|(_, d)| *d).collect();
            assert_eq!(metres, vec![50.0, 100.0, 200.0], "radius {radius}");
            assert!(scale.unlabelled.is_empty());
        }
    }

    #[test]
    fn at_another_zoom_the_distances_read_give_another_scale() {
        // The same rings labelled for a closer zoom: the scale doubles.
        let scale = scale_of(113, &RINGS, &["100 m", "200 m", "400 m"]).expect("a scale");
        assert!(
            (scale.metres_per_pixel / (400.0 / 103.0) - 1.0).abs() < 0.01,
            "{scale:?}"
        );
        assert_eq!(scale.labels, vec!["100 m200 m", "400 m"]);
    }

    #[test]
    fn touching_labels_are_read_as_one_and_shared_among_their_rings() {
        // The two inner rings' labels run into each other, as they do in
        // the client, and the outer ring's label is missing (in the client
        // the compass letter can stand over it).
        let scale = scale_of(113, &RINGS, &["50 m", "100 m"]).expect("a scale");
        assert_eq!(scale.labels, vec!["50 m100 m"]);
        assert_eq!(
            scale.rings.iter().map(|(_, d)| *d).collect::<Vec<_>>(),
            vec![50.0, 100.0]
        );
        assert_eq!(scale.unlabelled.len(), 1);
    }

    #[test]
    fn rings_without_distances_that_fit_them_give_no_scale() {
        for (rings, labels) in [
            // No distances printed; only one; in the wrong order; in
            // another proportion than the rings; not distances at all.
            (&RINGS[..], &[][..]),
            (&RINGS[..], &["50 m"][..]),
            (&RINGS[..], &["100 m", "50 m", "200 m"][..]),
            (&RINGS[..], &["50 m", "100 m", "300 m"][..]),
            (&[26.0, 40.0, 103.0][..], &LABELS[..]),
            (&RINGS[..], &["50", "100", "200"][..]),
        ] {
            assert_eq!(scale_of(113, rings, labels), None, "{rings:?} {labels:?}");
        }
    }

    #[test]
    fn a_ring_at_the_end_of_the_searched_span_gives_no_scale() {
        // Its brightness may carry on past the span, which would pull its
        // measured radius inward.
        let span_end = bundled_profile().radar.rings.span[1] * 113.0;
        let rings = [26.0, 52.0, span_end.floor()];
        assert_eq!(scale_of(113, &rings, &LABELS), None);
    }

    #[test]
    fn a_window_over_part_of_the_map_is_left_out_of_the_rings() {
        let profile = bundled_profile();
        let mut scene = RadarScene::new(77);
        let (centre, radius) = ((640, 380), 113);
        draw_radar(&mut scene.image, centre, radius, &RINGS, true);
        for (ring, label) in RINGS.iter().zip(LABELS) {
            scene.ring_label(centre, *ring, label);
        }
        // Over the lower left of the map, half its circles' points.
        draw_window(&mut scene.image, Rect::new(300, 380, centre.0 - 20, 700));
        let radar = find_radar(&scene.image, &profile).expect("the radar is found");
        let scale =
            ring_scale(&scene.image, &radar, &profile, &scene.recogniser()).expect("a scale");
        assert!(
            (scale.metres_per_pixel / (200.0 / 103.0) - 1.0).abs() < 0.01,
            "{scale:?}"
        );
    }

    #[test]
    fn a_profile_without_a_brightness_floor_gives_no_scale_rather_than_nonsense() {
        let mut profile = bundled_profile();
        profile.radar.rings.prominence = 0.0;
        let mut scene = RadarScene::new(78);
        draw_radar(&mut scene.image, (640, 380), 113, &RINGS, true);
        for (ring, label) in RINGS.iter().zip(LABELS) {
            scene.ring_label((640, 380), *ring, label);
        }
        // The same picture gives a scale with the profile's floor.
        let radar = find_radar(&scene.image, &bundled_profile()).expect("the radar is found");
        assert!(ring_scale(
            &scene.image,
            &radar,
            &bundled_profile(),
            &scene.recogniser()
        )
        .is_some());
        let radar = find_radar(&scene.image, &profile).expect("the radar is found");
        assert_eq!(
            ring_scale(&scene.image, &radar, &profile, &scene.recogniser()),
            None
        );
        let nowhere = Radar {
            x: f64::NAN,
            y: f64::NAN,
            radius: 113.0,
        };
        assert_eq!(
            ring_scale(
                &scene.image,
                &nowhere,
                &bundled_profile(),
                &scene.recogniser()
            ),
            None
        );
    }

    // Coordinates.

    /// Bounds that hold every drawn value.
    const ANYWHERE: CoordinateBounds = CoordinateBounds {
        lon: (0, 999_999),
        lat: (0, 999_999),
    };

    fn scene_with_coordinates(centre: (i32, i32), radius: i32, lon: &str, lat: &str) -> RadarScene {
        RadarScene::with_coordinates(
            centre,
            radius,
            Some((lon, Printed::of(5))),
            Some((lat, Printed::of(5))),
        )
    }

    fn read_within(
        scene: &RadarScene,
        bounds: &CoordinateBounds,
    ) -> Result<Coordinates, CoordError> {
        read_coordinates(
            &scene.image,
            &bundled_profile(),
            bounds,
            &scene.recogniser(),
        )
    }

    fn read(scene: &RadarScene) -> Result<Coordinates, CoordError> {
        read_within(scene, &ANYWHERE)
    }

    fn values(c: &Coordinates) -> (Result<u32, CoordError>, Result<u32, CoordError>) {
        (
            c.lon.as_ref().map(|v| v.value).map_err(Clone::clone),
            c.lat.as_ref().map(|v| v.value).map_err(Clone::clone),
        )
    }

    #[test]
    fn both_coordinates_read_under_a_radar_of_any_size() {
        for (radius, centre) in [
            (97, (1100, 140)),
            (113, (1080, 160)),
            (169, (640, 200)),
            (230, (400, 280)),
        ] {
            let scene = scene_with_coordinates(centre, radius, "LON 67739", "LAT 87740");
            let got = read(&scene).unwrap_or_else(|e| panic!("radius {radius}: {e}"));
            assert_eq!(values(&got), (Ok(67739), Ok(87740)), "radius {radius}");
            assert_eq!(got.lon.as_ref().map(|c| c.text.as_str()), Ok("LON 67739"));
        }
    }

    #[test]
    fn a_reading_a_digit_short_or_long_of_its_lettering_is_refused_on_its_own() {
        // The other value is not on screen, so only each value's own
        // width can tell.
        for (lon, want) in [("LON 6773", "LON 6773"), ("LON 677399", "LON 677399")] {
            let scene =
                RadarScene::with_coordinates((900, 200), 113, Some((lon, Printed::of(5))), None);
            let got = read(&scene).expect("the radar is found");
            assert_eq!(got.lon, Err(CoordError::WrongWidth { raw: want.into() }));
            assert_eq!(got.lat, Err(CoordError::NotFound));
        }
        let scene =
            RadarScene::with_coordinates((900, 200), 169, None, Some(("LAT 8774", Printed::of(5))));
        let got = read(&scene).expect("the radar is found");
        assert_eq!(
            got.lat,
            Err(CoordError::WrongWidth {
                raw: "LAT 8774".into()
            })
        );
    }

    #[test]
    fn a_whole_reading_of_cut_lettering_is_refused() {
        // Lettering with only four digits in view, read as five.
        let scene = RadarScene::with_coordinates(
            (900, 200),
            113,
            Some(("LON 67739", Printed::of(4))),
            None,
        );
        let got = read(&scene).expect("the radar is found");
        assert_eq!(
            got.lon,
            Err(CoordError::WrongWidth {
                raw: "LON 67739".into()
            })
        );
        // LAT missing its last digit at the disc's edge: read as the four
        // that show, its end is a digit short of the edge.
        let coords = bundled_profile().radar.coordinates;
        let (centre, radius) = ((900, 200), 113);
        let mut scene = RadarScene::new(5);
        draw_radar(&mut scene.image, centre, radius, &RINGS, true);
        let drawn = Printed::of(4);
        let right = centre.0 + radius - (0.6 * PITCH as f64).round() as i32 - PITCH;
        let width = RadarScene::width_of(&coords.lat, drawn);
        scene.value(
            right - width + 1,
            centre.1 + radius + 30,
            &coords.lat,
            drawn,
            "LAT 8774",
        );
        let got = read(&scene).expect("the radar is found");
        assert_eq!(
            got.lat,
            Err(CoordError::Misplaced {
                raw: "LAT 8774".into()
            })
        );
    }

    #[test]
    fn lettering_widened_by_the_scene_is_refused() {
        let mut scene = RadarScene::with_coordinates(
            (900, 200),
            113,
            Some(("LON 67739", Printed::of(5))),
            None,
        );
        // A pale speck with shadow just past the last digit joins the run.
        let coords = bundled_profile().radar.coordinates;
        let top = 200 + 113 + 30;
        let end = 900 - 113 + RadarScene::width_of(&coords.lon, Printed::of(5));
        fill(
            &mut scene.image,
            Rect::new(end + 2, top + 2, end + 7, top + 8),
            SHADOW,
        );
        fill(
            &mut scene.image,
            Rect::new(end + 4, top + 4, end + 4, top + 6),
            [240, 240, 240],
        );
        let got = read(&scene).expect("the radar is found");
        assert_eq!(
            got.lon,
            Err(CoordError::WrongWidth {
                raw: "LON 67739".into()
            })
        );
    }

    #[test]
    fn a_label_run_into_its_digits_is_not_measured() {
        let drawn = Printed {
            space: false,
            ..Printed::of(5)
        };
        let scene = RadarScene::with_coordinates((900, 200), 113, Some(("LON 67739", drawn)), None);
        let got = read(&scene).expect("the radar is found");
        assert_eq!(
            got.lon,
            Err(CoordError::NoSpace {
                raw: "LON 67739".into()
            })
        );
    }

    #[test]
    fn a_value_outside_the_planets_coordinates_is_refused() {
        // Lettering cut short by something that is not a window: four
        // digits drawn and read, consistent with each other.
        let scene = RadarScene::with_coordinates(
            (900, 200),
            113,
            Some(("LON 6773", Printed::of(4))),
            Some(("LAT 87740", Printed::of(5))),
        );
        let planet = CoordinateBounds {
            lon: (16384, 90112),
            lat: (24576, 98304),
        };
        let got = read_within(&scene, &planet).expect("the radar is found");
        assert_eq!(
            values(&got),
            (
                Err(CoordError::OutOfBounds {
                    text: "LON 6773".into()
                }),
                Ok(87740)
            )
        );
        assert_eq!(values(&read(&scene).expect("read")).0, Ok(6773));
    }

    #[test]
    fn values_drawn_in_two_sizes_disagree_with_each_other() {
        // Each is as wide as its own label says, but the client prints
        // both in one font.
        let big = Printed {
            pitch: 10,
            ..Printed::of(5)
        };
        let scene = RadarScene::with_coordinates(
            (900, 200),
            169,
            Some(("LON 67739", Printed::of(5))),
            Some(("LAT 87740", big)),
        );
        let got = read(&scene).expect("the radar is found");
        let refusal = CoordError::WidthsDisagree {
            lon: "LON 67739".into(),
            lat: "LAT 87740".into(),
        };
        assert_eq!((got.lon, got.lat), (Err(refusal.clone()), Err(refusal)));
    }

    #[test]
    fn a_value_under_a_window_is_refused_and_the_other_still_read() {
        let (centre, radius) = ((900, 200), 169);
        let mut scene = scene_with_coordinates(centre, radius, "LON 68520", "LAT 87611");
        // A window whose right edge lies over the disc's left side and the
        // start of the band under it.
        draw_window(
            &mut scene.image,
            Rect::new(300, 40, centre.0 - radius + 80, 650),
        );
        let got = read(&scene).expect("the radar is found");
        assert_eq!(values(&got), (Err(CoordError::Covered), Ok(87611)));
    }

    #[test]
    fn a_window_beside_the_digits_is_refused_and_one_beside_the_label_is_not() {
        let (centre, radius) = ((700, 200), 169);
        let top = centre.1 + radius + 20;
        let scene = || scene_with_coordinates(centre, radius, "LON 68586", "LAT 87917");
        let drawn = read(&scene()).expect("the radar is found");
        assert_eq!(values(&drawn), (Ok(68586), Ok(87917)));
        let lon = drawn.lon.as_ref().expect("read").rect;
        let lat = drawn.lat.as_ref().expect("read").rect;
        for (front, want) in [
            // Ending a few pixels short of either label: nothing of the
            // digits can be under it.
            (Rect::new(200, 40, lon.x0 - 4, 650), (Ok(68586), Ok(87917))),
            (
                Rect::new(lon.x1 + 20, top, lat.x0 - 4, 650),
                (Ok(68586), Ok(87917)),
            ),
            // Beginning a few pixels past LON's last digit (and covering
            // LAT), or over the end of LAT's last digit.
            (
                Rect::new(lon.x1 + 4, top, lon.x1 + 300, 650),
                (Err(CoordError::Covered), Err(CoordError::Covered)),
            ),
            (
                Rect::new(lat.x1 - 3, top, lat.x1 + 200, 650),
                (Ok(68586), Err(CoordError::Covered)),
            ),
            // Along the top of LON's lettering.
            (
                Rect::new(lon.x0 - 200, lon.y0 - 110, lon.x1 - 20, lon.y0 + 3),
                (Err(CoordError::Covered), Ok(87917)),
            ),
        ] {
            let mut scene = scene();
            draw_window(&mut scene.image, front);
            assert_eq!(
                values(&read(&scene).expect("the radar is found")),
                want,
                "{front:?}"
            );
        }
    }

    #[test]
    fn a_band_run_off_the_screen_is_refused() {
        let scene = scene_with_coordinates((640, 600), 113, "LON 67739", "LAT 87740");
        let got = read(&scene).expect("the radar is found");
        assert_eq!(
            values(&got),
            (Err(CoordError::OffScreen), Err(CoordError::OffScreen))
        );
    }

    #[test]
    fn a_reading_that_is_not_the_label_and_one_run_of_digits_is_refused() {
        for lon in [
            "L0N 67739",
            "LON 677 39",
            "LON 677a9",
            "LAT 67739",
            "LON 673",
        ] {
            let scene = scene_with_coordinates((900, 200), 113, lon, "LAT 87740");
            let got = read(&scene).expect("the radar is found");
            assert_eq!(
                got.lon,
                Err(CoordError::Invalid { raw: lon.into() }),
                "{lon:?}"
            );
            assert_eq!(values(&got).1, Ok(87740));
        }
    }

    #[test]
    fn lettering_out_of_place_is_not_a_coordinate() {
        let coords = bundled_profile().radar.coordinates;
        let (centre, radius) = ((900, 200), 113);
        let top = centre.1 + radius + 30;
        // Nothing printed; then lettering two heights in from the disc's
        // edge; then lettering running into the other half of the band.
        let mut nothing = RadarScene::new(1);
        draw_radar(&mut nothing.image, centre, radius, &RINGS, true);
        let mut shifted = RadarScene::new(2);
        draw_radar(&mut shifted.image, centre, radius, &RINGS, true);
        shifted.value(
            centre.0 - radius + 2 * HEIGHT,
            top,
            &coords.lon,
            Printed::of(5),
            "LON 67739",
        );
        let mut long = RadarScene::new(3);
        draw_radar(&mut long.image, centre, radius, &RINGS, true);
        long.value(
            centre.0 - radius,
            top,
            &coords.lon,
            Printed::of(14),
            "LON 67739",
        );
        for (scene, want) in [
            (&nothing, CoordError::NotFound),
            (&shifted, CoordError::NotFound),
            (&long, CoordError::Clipped),
        ] {
            assert_eq!(read(scene).expect("the radar is found").lon, Err(want));
        }
    }

    #[test]
    fn without_a_radar_or_a_recogniser_nothing_is_read() {
        let scene = RadarScene::new(4);
        assert_eq!(read(&scene), Err(CoordError::RadarNotFound));
        let scene = scene_with_coordinates((900, 200), 113, "LON 67739", "LAT 87740");
        let none = |_: &BgrImage| -> Option<(String, f64)> { None };
        assert_eq!(
            read_coordinates(&scene.image, &bundled_profile(), &ANYWHERE, &none),
            Err(CoordError::NoRecogniser)
        );
    }

    // The planet's name.

    const PLANETS: [&str; 3] = ["Calypso", "Arkadia", "Arkadia Moon"];

    fn label_scene(text: Option<&str>) -> (RadarScene, Radar) {
        let (centre, radius) = ((640, 380), 113);
        let mut scene = RadarScene::new(31);
        scene.radar(centre, radius, &RINGS);
        if let Some(text) = text {
            scene.planet_label(centre, radius, text);
        }
        let radar = find_radar(&scene.image, &bundled_profile()).expect("the radar is found");
        (scene, radar)
    }

    fn label_of(scene: &RadarScene, radar: &Radar) -> Option<String> {
        read_planet_label(
            &scene.image,
            radar,
            &bundled_profile(),
            &PLANETS,
            &scene.recogniser(),
        )
    }

    #[test]
    fn the_planets_name_above_the_radar_reads_as_the_planet_listed() {
        let (scene, radar) = label_scene(Some("CALYPSO"));
        assert_eq!(label_of(&scene, &radar), Some("Calypso".to_string()));
        let (scene, radar) = label_scene(Some("Arkadia  Moon"));
        assert_eq!(label_of(&scene, &radar), Some("Arkadia Moon".to_string()));
    }

    #[test]
    fn a_name_that_is_not_a_planet_or_not_shown_reads_as_nothing() {
        for text in [None, Some("Calyps0"), Some("Caly"), Some("Monria")] {
            let (scene, radar) = label_scene(text);
            assert_eq!(label_of(&scene, &radar), None, "{text:?}");
        }
    }

    #[test]
    fn a_name_under_a_window_reads_as_nothing() {
        let (mut scene, radar) = label_scene(Some("Calypso"));
        draw_window(&mut scene.image, Rect::new(300, 100, 650, 250));
        assert_eq!(label_of(&scene, &radar), None);
    }

    #[test]
    fn rings_are_told_from_their_labels() {
        let (scene, radar) = label_scene(None);
        assert!(rings_found(&scene.image, &radar, &bundled_profile()));
        let mut plain = RadarScene::new(32);
        plain.radar((640, 380), 113, &[]);
        let radar = find_radar(&plain.image, &bundled_profile()).expect("the radar is found");
        assert!(!rings_found(&plain.image, &radar, &bundled_profile()));
    }
}

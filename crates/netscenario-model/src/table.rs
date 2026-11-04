//! The table of a jitter distribution made from samples (LNK-02, D-083, D-084).
//!
//! `netem` draws the variation of a delay from a table of 4096 numbers: the delay is `delay + amount * entry / 8192`, with
//! an entry chosen at random. The tables that ship with iproute2 (`normal`, `pareto`, `paretonormal`) are the inverse of a
//! distribution function with a mean of about 0 and a spread of about 1. A table of our own is made from a file of samples
//! in the same way: the 4096 evenly spaced quantiles of the samples, moved to a mean of 0 and scaled to a standard deviation
//! of 1, times 8192. The table is only the *shape* of the distribution, so the unit of the numbers in the file does not
//! matter; `delay` and `amount` of the jitter place and scale it.
//!
//! The entries are 16 bits, so a table reaches at most 4 standard deviations from its mean. Samples that reach further (a
//! long tail, which is what the delays of a real network have) are not cut: the entries are scaled down by a factor and the
//! `amount` that goes to `netem` is scaled up by the same factor, so that the delays come out as the samples have them. The
//! factor is the `scale` of the table (in thousandths, 1000 for a table that fits).

use std::fmt::{self, Write as _};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::scenario::Jitter;

/// The entries of a table.
pub const TABLE_ENTRIES: usize = 4096;
/// An entry of `netem` that is this large is one standard deviation.
pub const TABLE_SCALE: i32 = 8192;
/// A file of samples has at least this many: fewer say nothing about the shape of a distribution.
pub const MIN_SAMPLES: usize = 100;
/// A file of samples has at most this many.
pub const MAX_SAMPLES: usize = 1_000_000;
/// A file that a scenario refers to is at most this large (D-083).
pub const MAX_REFERENCED_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// The delay table of a jitter with `distribution: table`: where the samples came from and the table made of them.
///
/// In a scenario file only `file` is written. When the scenario is loaded the file is read and the other fields are filled
/// in, so that the canonical form of the scenario (and its hash) holds the table and the fingerprint of the file, and a
/// recorded scenario runs without the file (D-083). `sha256` may be written as well: then the file must have exactly that
/// content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DelayTable {
    /// The file of samples, one number per line (or several, separated by spaces or commas), a path relative to the
    /// directory of the scenario file. Lines that start with `#` are comments. Read when the scenario is loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The SHA-256 of the file. Written by the loader; if it is in the file, the content of the file must match it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// How many samples the table was made from. Written by the loader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples: Option<u32>,
    /// The factor, in thousandths, by which the entries were scaled down so that they fit 16 bits, and by which the `amount` of
    /// the jitter is scaled up when it goes to `netem`. 1000 (or left out) when the samples reach at most 4 standard
    /// deviations. Written by the loader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
    /// The 4096 entries, as 4 hexadecimal digits each. Written by the loader; a scenario with the entries needs no file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<TableValues>,
}

impl DelayTable {
    /// A reference to a file that is not read yet.
    #[must_use]
    pub fn from_file(file: &str) -> Self {
        Self {
            file: Some(file.to_owned()),
            sha256: None,
            samples: None,
            scale: None,
            values: None,
        }
    }

    /// A table that holds its entries, without a file.
    #[must_use]
    pub fn from_values(values: TableValues) -> Self {
        Self {
            file: None,
            sha256: None,
            samples: None,
            scale: None,
            values: Some(values),
        }
    }

    /// The factor by which the `amount` goes to `netem`, in thousandths (1000 when the table was not scaled).
    #[must_use]
    pub fn scale_thousandths(&self) -> u32 {
        self.scale.unwrap_or(1000).max(1)
    }

    /// How far below the mean the table reaches, in `amount`s: the delay has to be at least this many times the jitter. `None`
    /// until the entries are there.
    #[must_use]
    pub fn reach_below(&self) -> Option<f64> {
        self.values
            .as_ref()
            .map(|values| values.reach_below() * f64::from(self.scale_thousandths()) / 1000.0)
    }

    /// Whether the entries are there.
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.values.is_some()
    }
}

/// The entries of a table, in the order of `netem`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct TableValues(Vec<i16>);

impl fmt::Debug for TableValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TableValues({} entries)", self.0.len())
    }
}

impl TableValues {
    /// The entries.
    #[must_use]
    pub fn entries(&self) -> &[i16] {
        &self.0
    }

    /// How far below the mean the table reaches, in standard deviations: the delay has to be at least this many times the
    /// `amount` of the jitter, or the shortest delays would be below zero.
    #[must_use]
    pub fn reach_below(&self) -> f64 {
        let lowest = self.0.iter().copied().min().unwrap_or(0);
        (-f64::from(lowest) / f64::from(TABLE_SCALE)).max(0.0)
    }

    /// The entries as the file of `tc` has them: a comment line, then eight numbers to a line.
    #[must_use]
    pub fn to_dist_file(&self, comment: &str) -> String {
        let mut text = format!("# {comment}\n");
        for line in self.0.chunks(8) {
            let numbers: Vec<String> = line.iter().map(ToString::to_string).collect();
            text.push(' ');
            text.push_str(&numbers.join(" "));
            text.push('\n');
        }
        text
    }

    /// Takes the entries from a table that `tc` reads (the lines that start with `#` are comments). Used for the tables of the
    /// system, so that a test can compare a table with what the kernel draws from.
    ///
    /// # Errors
    ///
    /// A sentence if a number is not an integer of 16 bits or the number of entries is not 4096.
    pub fn from_dist_file(text: &str) -> Result<Self, String> {
        let mut entries = Vec::new();
        for token in text
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .flat_map(str::split_whitespace)
        {
            entries.push(
                token
                    .parse::<i16>()
                    .map_err(|_| format!("'{token}' is not an entry of a table"))?,
            );
        }
        if entries.len() != TABLE_ENTRIES {
            return Err(format!(
                "a table has {TABLE_ENTRIES} entries, this one has {}",
                entries.len()
            ));
        }
        Ok(Self(entries))
    }

    /// Makes the table of the samples. See the module description for how. Returns the entries and the `scale`, in thousandths:
    /// 1000 if the samples reach at most four standard deviations from their mean, else the factor that brings them there.
    ///
    /// # Errors
    ///
    /// A sentence if there are too few samples or if they do not vary.
    pub fn from_samples(samples: &[f64]) -> Result<(Self, u32), String> {
        if samples.len() < MIN_SAMPLES {
            return Err(format!(
                "{} samples are too few: a table needs at least {MIN_SAMPLES} to say anything about the shape of a distribution",
                samples.len()
            ));
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let last = sorted.len() - 1;
        // The quantile at (i + 1/2) / 4096, with a straight line between the two samples around it
        #[allow(clippy::cast_precision_loss)]
        let quantiles: Vec<f64> = (0..TABLE_ENTRIES)
            .map(|i| {
                let at = ((i as f64 + 0.5) / TABLE_ENTRIES as f64 * sorted.len() as f64 - 0.5)
                    .clamp(0.0, last as f64);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let below = at.floor() as usize;
                let above = (below + 1).min(last);
                #[allow(clippy::cast_precision_loss)]
                let part = at - below as f64;
                sorted[below] + (sorted[above] - sorted[below]) * part
            })
            .collect();
        #[allow(clippy::cast_precision_loss)]
        let count = quantiles.len() as f64;
        let mean = quantiles.iter().sum::<f64>() / count;
        let deviation = (quantiles.iter().map(|q| (q - mean).powi(2)).sum::<f64>() / count).sqrt();
        if !deviation.is_finite() || deviation <= 0.0 {
            return Err(
                "the samples do not vary, so there is no distribution to copy: use `delay` without a `jitter`"
                    .to_owned(),
            );
        }
        let farthest = quantiles
            .iter()
            .map(|q| ((q - mean) / deviation).abs())
            .fold(0.0f64, f64::max);
        // The entries reach 32767 / 8192 = 3.9999 standard deviations; what is further is brought in by a factor, rounded up
        // to the thousandth so that the largest entry stays within 16 bits
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let scale = ((farthest / (f64::from(i16::MAX) / f64::from(TABLE_SCALE)) * 1000.0).ceil()
            as u32)
            .max(1000);
        // 4096 entries of equal chance cannot hold one further than sqrt(4095) = 64 standard deviations, so the scale is at most
        // 16 and the entries stay finer than 0.002 standard deviations
        debug_assert!(scale <= 16_100, "scale {scale}");
        let factor = f64::from(scale) / 1000.0;
        let entries = quantiles
            .iter()
            .map(|quantile| {
                let scaled =
                    ((quantile - mean) / deviation / factor * f64::from(TABLE_SCALE)).round();
                #[allow(clippy::cast_possible_truncation)]
                {
                    scaled.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
                }
            })
            .collect();
        Ok((Self(entries), scale))
    }
}

/// Why a path in a scenario cannot be a file, if it cannot (D-083). Where a file may be is decided when it is read.
#[must_use]
pub(crate) fn path_problem(path: &str) -> Option<String> {
    if path.is_empty() {
        Some("the path of the file is empty".to_owned())
    } else if path.contains('\0') || path.len() > 4096 {
        Some("the path of the file is not a path".to_owned())
    } else {
        None
    }
}

/// The numbers in a file of samples: one or more to a line, separated by spaces or commas. Lines that start with `#` and empty
/// lines are skipped.
///
/// # Errors
///
/// A sentence with the line if something is not a finite number or there are more samples than [`MAX_SAMPLES`].
pub fn parse_samples(text: &str) -> Result<Vec<f64>, String> {
    let mut samples = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for token in line
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|token| !token.is_empty())
        {
            let number: f64 = token
                .parse()
                .map_err(|_| format!("line {}: '{token}' is not a number", index + 1))?;
            if !number.is_finite() {
                return Err(format!(
                    "line {}: '{token}' is not a finite number",
                    index + 1
                ));
            }
            samples.push(number);
            if samples.len() > MAX_SAMPLES {
                return Err(format!(
                    "more than {MAX_SAMPLES} samples: a table does not need that many"
                ));
            }
        }
    }
    Ok(samples)
}

impl Serialize for TableValues {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut text = String::with_capacity(self.0.len() * 4);
        for value in &self.0 {
            // The two's complement of the entry, four hexadecimal digits
            #[allow(clippy::cast_sign_loss)]
            let _ = write!(text, "{:04x}", *value as u16);
        }
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for TableValues {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.len() != TABLE_ENTRIES * 4 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(serde::de::Error::custom(format!(
                "the entries of a table are {TABLE_ENTRIES} numbers of 4 hexadecimal digits each ({} characters), this has {} characters",
                TABLE_ENTRIES * 4,
                text.len()
            )));
        }
        let mut entries = Vec::with_capacity(TABLE_ENTRIES);
        for chunk in text.as_bytes().chunks(4) {
            let digits = std::str::from_utf8(chunk).map_err(serde::de::Error::custom)?;
            let value = u16::from_str_radix(digits, 16).map_err(serde::de::Error::custom)?;
            #[allow(clippy::cast_possible_wrap)]
            entries.push(value as i16);
        }
        Ok(Self(entries))
    }
}

impl JsonSchema for TableValues {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TableValues".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^[0-9a-fA-F]{16384}$",
            "description": "The 4096 entries of a delay table, each as 4 hexadecimal digits (two's complement, 8192 is one standard deviation). Written by the loader from the file of samples."
        })
    }
}

impl crate::scenario::Scenario {
    /// The files that tables of this scenario still have to read: each `file` once, in the order of the scenario.
    #[must_use]
    pub fn unresolved_table_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        for link in &self.topology.links {
            for impairments in [&link.forward, &link.reverse, &link.both]
                .into_iter()
                .flatten()
            {
                if let Some(Jitter {
                    table: Some(table), ..
                }) = &impairments.jitter
                    && table.values.is_none()
                    && let Some(file) = &table.file
                    && !files.contains(file)
                {
                    files.push(file.clone());
                }
            }
        }
        files
    }

    /// Fills in the tables that refer to a file (D-083, D-084): `read` gives the content of a file by the path written in the
    /// scenario. The tables get their entries, the number of samples and the SHA-256 of the file, so that the scenario holds
    /// everything that it runs with. A table with a `sha256` of its own is checked against the file.
    ///
    /// Returns the problems that were found, with the path of each table in the scenario; the scenario is left as it was
    /// when there are any.
    pub fn resolve_tables(
        &mut self,
        mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
    ) -> Vec<crate::error::Problem> {
        use std::collections::HashMap;
        let mut problems = Vec::new();
        // Each file is read and turned into a table once, even if several links name it
        let mut made: HashMap<String, Result<(DelayTable, usize), String>> = HashMap::new();
        let mut resolved = self.clone();
        for (index, link) in resolved.topology.links.iter_mut().enumerate() {
            for (name, impairments) in [
                ("forward", &mut link.forward),
                ("reverse", &mut link.reverse),
                ("both", &mut link.both),
            ] {
                let Some(Jitter {
                    table: Some(table), ..
                }) = impairments.as_mut().and_then(|i| i.jitter.as_mut())
                else {
                    continue;
                };
                if table.values.is_some() {
                    continue;
                }
                let Some(file) = table.file.clone() else {
                    continue;
                };
                let path = format!("topology.links[{index}].{name}.jitter.table.file");
                let entry = made.entry(file.clone()).or_insert_with(|| {
                    read(&file).and_then(|bytes| {
                        let hash = crate::hash::sha256_hex(&bytes);
                        let text = String::from_utf8(bytes)
                            .map_err(|_| format!("'{file}' is not text in UTF-8"))?;
                        let samples =
                            parse_samples(&text).map_err(|why| format!("'{file}': {why}"))?;
                        let (values, scale) = TableValues::from_samples(&samples)
                            .map_err(|why| format!("'{file}': {why}"))?;
                        let count = samples.len();
                        Ok((
                            DelayTable {
                                file: Some(file.clone()),
                                sha256: Some(hash),
                                samples: u32::try_from(count).ok(),
                                scale: (scale != 1000).then_some(scale),
                                values: Some(values),
                            },
                            count,
                        ))
                    })
                });
                match entry {
                    Ok((made, _)) => {
                        if let Some(pinned) = &table.sha256
                            && Some(pinned.to_ascii_lowercase()) != made.sha256
                        {
                            problems.push(
                                crate::error::Problem::new(format!(
                                    "'{file}' is not the file the scenario was written for: its SHA-256 is {}, the scenario says {pinned}",
                                    made.sha256.as_deref().unwrap_or("unknown")
                                ))
                                .at_path(path)
                                .with_help("the file changed; check it, then write the new `sha256` or take it away"),
                            );
                            continue;
                        }
                        *table = made.clone();
                    }
                    Err(why) => problems.push(
                        crate::error::Problem::new(why.clone())
                            .at_path(path)
                            .with_help(
                                "a file of samples has one delay per line, at least 100 of them",
                            ),
                    ),
                }
            }
        }
        if problems.is_empty() {
            // The reach of the table below the delay is known now
            problems.extend(resolved.validate());
        }
        if problems.is_empty() {
            *self = resolved;
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic set of samples with a known shape: `n` values of `shape(u)` at evenly spaced `u` in (0, 1).
    fn samples_of(n: usize, shape: impl Fn(f64) -> f64) -> Vec<f64> {
        #[allow(clippy::cast_precision_loss)]
        (0..n).map(|i| shape((i as f64 + 0.5) / n as f64)).collect()
    }

    /// The mean and the standard deviation of the table in units of `amount`: entries / 8192 times the scale.
    fn moments(values: &TableValues, scale: u32) -> (f64, f64) {
        let numbers: Vec<f64> = values
            .entries()
            .iter()
            .map(|v| f64::from(*v) / f64::from(TABLE_SCALE) * f64::from(scale) / 1000.0)
            .collect();
        #[allow(clippy::cast_precision_loss)]
        let n = numbers.len() as f64;
        let mean = numbers.iter().sum::<f64>() / n;
        let var = numbers.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
        (mean, var.sqrt())
    }

    /// A table made of samples has the mean 0 and the standard deviation 1 that `netem` takes a table to have, whatever the
    /// location and the scale of the samples (the unit of the file does not matter). The rounding to integers of 1/8192 moves
    /// them by less than 1e-3.
    #[test]
    fn a_table_is_the_shape_of_the_samples_with_mean_0_and_deviation_1() {
        for (location, scale) in [(0.0, 1.0), (30.0, 2.5), (-5.0, 1000.0)] {
            let samples = samples_of(2000, |u| location + scale * (u - 0.5));
            let (table, factor) = TableValues::from_samples(&samples).unwrap();
            assert_eq!(table.entries().len(), TABLE_ENTRIES);
            assert_eq!(
                factor, 1000,
                "a uniform distribution reaches 1.73 deviations"
            );
            let (mean, deviation) = moments(&table, factor);
            assert!(mean.abs() < 1e-3, "mean {mean} for {location}, {scale}");
            assert!(
                (deviation - 1.0).abs() < 1e-3,
                "deviation {deviation} for {location}, {scale}"
            );
        }
    }

    /// The table is sorted (it is the inverse of a distribution function), and the samples of a uniform distribution give a
    /// straight line: entry `i` is `sqrt(12) * ((i + 1/2) / 4096 - 1/2)` standard deviations, to the rounding.
    #[test]
    fn the_table_of_uniform_samples_is_a_straight_line() {
        let (table, _) = TableValues::from_samples(&samples_of(4000, |u| 10.0 + 4.0 * u)).unwrap();
        assert!(table.entries().windows(2).all(|pair| pair[0] <= pair[1]));
        let slope = 12.0f64.sqrt();
        for (i, entry) in table.entries().iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let expected =
                slope * ((i as f64 + 0.5) / TABLE_ENTRIES as f64 - 0.5) * f64::from(TABLE_SCALE);
            assert!(
                (f64::from(*entry) - expected).abs() <= 8.0,
                "entry {i}: {entry}, the line says {expected:.1}"
            );
        }
        // A uniform distribution reaches 1.73 standard deviations, so a delay of that many amounts is enough
        assert!((table.reach_below() - slope / 2.0).abs() < 0.01);
    }

    /// Canary: the check of the shape can fail. Samples of one spread but another shape give another table, so a test that
    /// compared only the moments would not see it.
    #[test]
    fn canary_two_shapes_with_the_same_moments_give_different_tables() {
        let (uniform, uniform_scale) = TableValues::from_samples(&samples_of(2000, |u| u)).unwrap();
        // A bell with a tail on both sides: the logistic distribution
        let (bell, bell_scale) =
            TableValues::from_samples(&samples_of(2000, |u| (u / (1.0 - u)).ln())).unwrap();
        // They are compared in units of the amount, where both have the deviation 1
        let in_amounts = |values: &TableValues, scale: u32| -> Vec<f64> {
            values
                .entries()
                .iter()
                .map(|v| f64::from(*v) / f64::from(TABLE_SCALE) * f64::from(scale) / 1000.0)
                .collect()
        };
        let difference = in_amounts(&uniform, uniform_scale)
            .iter()
            .zip(in_amounts(&bell, bell_scale))
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f64, f64::max);
        assert!(
            difference > 1.0,
            "the tables differ by {difference} amounts at most"
        );
    }

    /// A distribution with a tail reaches further than four deviations, which 16 bits cannot hold. The table is scaled down and
    /// the scale says by how much, so nothing is cut: in units of the amount the table still has the mean 0 and the deviation
    /// 1, and its largest entry is the largest 16-bit number. The exponential distribution (the delays of a queue) reaches
    /// 8.0 deviations at the quantile 1 - 1/8192 (ln(8192) = 9.01, minus the mean of 1, over a deviation of 1).
    #[test]
    fn a_tail_beyond_four_deviations_is_not_cut_but_scaled() {
        let (table, scale) =
            TableValues::from_samples(&samples_of(100_000, |u| -(1.0 - u).ln())).unwrap();
        assert!(
            (2000..=2100).contains(&scale),
            "the scale is {scale}, the model says 8.0 / 3.9999 = 2.0"
        );
        let largest = table.entries().iter().copied().max().unwrap();
        assert!(largest >= 32_700, "{largest}");
        let (mean, deviation) = moments(&table, scale);
        assert!(mean.abs() < 2e-3, "{mean}");
        assert!((deviation - 1.0).abs() < 2e-3, "{deviation}");
        // Canary: the scale is what makes it so. Without it the deviation of the entries is the half of what it should be
        let (_, unscaled) = moments(&table, 1000);
        assert!((unscaled - 0.5).abs() < 0.01, "{unscaled}");
        // The reach below the mean is the one of the scaled table: the exponential has its smallest value 1 deviation below
        let table = DelayTable {
            scale: Some(scale),
            ..DelayTable::from_values(table)
        };
        let reach = table.reach_below().unwrap();
        assert!((reach - 1.0).abs() < 0.02, "{reach}");
    }

    /// Four deviations is what a table holds, and 4096 entries of equal chance cannot hold one further than 64: one outlier
    /// among the entries gives a scale of 16 and no more.
    #[test]
    fn the_scale_is_at_most_sixteen() {
        let mut samples = vec![0.0; 100_000];
        for sample in samples.iter_mut().rev().take(24) {
            *sample = 1e9;
        }
        let (table, scale) = TableValues::from_samples(&samples).unwrap();
        assert!((15_900..=16_100).contains(&scale), "{scale}");
        assert!(table.entries().iter().copied().max().unwrap() >= 32_700);
    }

    #[test]
    fn samples_that_cannot_make_a_table_are_refused_with_the_reason() {
        let few = TableValues::from_samples(&[1.0, 2.0, 3.0]).unwrap_err();
        assert!(few.contains("3 samples are too few"), "{few}");
        let flat = TableValues::from_samples(&vec![5.0; 200]).unwrap_err();
        assert!(flat.contains("do not vary"), "{flat}");
    }

    #[test]
    fn samples_are_read_from_lines_with_comments_and_separators() {
        let text = "# delays\n10.5\n\n 11 , 12\t13\n# end\n";
        assert_eq!(parse_samples(text).unwrap(), vec![10.5, 11.0, 12.0, 13.0]);
        let bad = parse_samples("1\n2\nabc\n").unwrap_err();
        assert_eq!(bad, "line 3: 'abc' is not a number");
        assert!(
            parse_samples("1\nNaN\n")
                .unwrap_err()
                .contains("not a finite number")
        );
        assert!(parse_samples("inf").unwrap_err().contains("line 1"));
    }

    #[test]
    fn the_entries_are_written_as_hexadecimal_and_read_back() {
        let (table, _) = TableValues::from_samples(&samples_of(1000, |u| u * u)).unwrap();
        let json = serde_json::to_string(&table).unwrap();
        assert_eq!(json.len(), TABLE_ENTRIES * 4 + 2);
        let back: TableValues = serde_json::from_str(&json).unwrap();
        assert_eq!(back, table);
        // A negative entry is written as its two's complement: the first entry of a table is the lowest
        assert!(table.entries()[0] < 0);
        #[allow(clippy::cast_sign_loss)]
        let first = format!("{:04x}", table.entries()[0] as u16);
        assert_eq!(&json[1..5], first);
        assert!(
            first.as_str() >= "8000",
            "a negative entry has the top bit set: {first}"
        );
        let short: Result<TableValues, _> = serde_json::from_str("\"00ff\"");
        assert!(short.unwrap_err().to_string().contains("16384 characters"));
    }

    /// The file for `tc` has the form of the files that ship with iproute2 and reads back to the same entries.
    #[test]
    fn the_file_for_tc_reads_back_to_the_same_entries() {
        let (table, _) = TableValues::from_samples(&samples_of(500, |u| u.powi(2))).unwrap();
        let file = table.to_dist_file("test");
        assert!(file.starts_with("# test\n "));
        assert_eq!(file.lines().count(), 1 + TABLE_ENTRIES / 8);
        assert_eq!(TableValues::from_dist_file(&file).unwrap(), table);
        assert!(
            TableValues::from_dist_file("1 2 3")
                .unwrap_err()
                .contains("4096 entries")
        );
    }
}

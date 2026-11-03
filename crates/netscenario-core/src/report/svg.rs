//! The drawings of the HTML report: SVG as text, with no script, no font, no image and no address.
//!
//! Every drawing is a function of its data: the same data gives the same bytes. The colours are not in the drawing but in
//! classes (`line-p50`, `bar-lost`, ...) that the page styles, so that the page can follow the light and the dark scheme of
//! the reader. Numbers are written with one decimal. The axes start at zero where the data are counts or times, and end on a
//! round number (1, 2 or 5 times a power of ten) above the data.

use std::fmt::Write as _;

use super::escape;

const WIDTH: f64 = 720.0;
const LEFT: f64 = 66.0;
const RIGHT: f64 = 14.0;
const TOP: f64 = 30.0;
const BOTTOM: f64 = 40.0;
const PLOT_WIDTH: f64 = WIDTH - LEFT - RIGHT;

/// A number with one decimal and no `-0.0`.
fn n(value: f64) -> String {
    let text = format!("{value:.1}");
    if text == "-0.0" {
        "0.0".to_owned()
    } else {
        text
    }
}

/// The step of the ticks of an axis that runs over `range`: 1, 2 or 5 times a power of ten, giving at most about `wanted` ticks.
fn nice_step(range: f64, wanted: f64) -> f64 {
    if range <= 0.0 || !range.is_finite() {
        return 1.0;
    }
    let raw = range / wanted;
    let magnitude = 10f64.powf(raw.log10().floor());
    let scaled = raw / magnitude;
    let factor = if scaled <= 1.0 {
        1.0
    } else if scaled <= 2.0 {
        2.0
    } else if scaled <= 5.0 {
        5.0
    } else {
        10.0
    };
    factor * magnitude
}

/// The top of an axis that holds `max`, on a multiple of the tick step, and the step.
fn nice_top(max: f64) -> (f64, f64) {
    if max <= 0.0 || !max.is_finite() {
        return (1.0, 0.2);
    }
    let step = nice_step(max, 5.0);
    ((max / step).ceil() * step, step)
}

/// A number for a tick label: no trailing zeros.
pub(crate) fn tick(value: f64) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" || text.is_empty() {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// The place of a drawing: where the plot lies in the picture and how the data map into it.
struct Frame {
    height: f64,
    x_min: f64,
    x_max: f64,
    y_max: f64,
}

impl Frame {
    fn plot_height(&self) -> f64 {
        self.height - TOP - BOTTOM
    }

    fn x(&self, value: f64) -> f64 {
        let span = (self.x_max - self.x_min).max(f64::EPSILON);
        LEFT + (value - self.x_min) / span * PLOT_WIDTH
    }

    fn y(&self, value: f64) -> f64 {
        TOP + self.plot_height() - value / self.y_max.max(f64::EPSILON) * self.plot_height()
    }
}

/// The opening tag, the title and the description, which make the picture readable for a screen reader.
fn open(out: &mut String, height: f64, title: &str, description: &str) {
    let _ = write!(
        out,
        "<svg viewBox=\"0 0 {} {}\" role=\"img\" aria-label=\"{}\"><title>{}</title><desc>{}</desc>",
        n(WIDTH),
        n(height),
        escape(title),
        escape(title),
        escape(description)
    );
}

/// The axes: horizontal grid lines with their labels, ticks and labels on the x axis, and the names of the axes.
fn axes(
    out: &mut String,
    frame: &Frame,
    x_label: &str,
    y_label: &str,
    y_format: &dyn Fn(f64) -> String,
    x_format: &dyn Fn(f64) -> String,
) {
    let (_, y_step) = nice_top(frame.y_max);
    let mut value = 0.0;
    while value <= frame.y_max + y_step * 1e-9 {
        let y = frame.y(value);
        let _ = write!(
            out,
            "<line class=\"grid\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/><text class=\"tick\" x=\"{}\" y=\"{}\" text-anchor=\"end\">{}</text>",
            n(LEFT),
            n(y),
            n(WIDTH - RIGHT),
            n(y),
            n(LEFT - 6.0),
            n(y + 3.5),
            escape(&y_format(value))
        );
        value += y_step;
    }
    let x_step = nice_step(frame.x_max - frame.x_min, 8.0);
    let mut value = (frame.x_min / x_step).ceil() * x_step;
    while value <= frame.x_max + x_step * 1e-9 {
        let x = frame.x(value);
        let baseline = frame.y(0.0);
        let _ = write!(
            out,
            "<line class=\"axis\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/><text class=\"tick\" x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
            n(x),
            n(baseline),
            n(x),
            n(baseline + 4.0),
            n(x),
            n(baseline + 16.0),
            escape(&x_format(value))
        );
        value += x_step;
    }
    let baseline = frame.y(0.0);
    let _ = write!(
        out,
        "<line class=\"axis\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/><text class=\"label\" x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text><text class=\"label\" x=\"14\" y=\"{}\" text-anchor=\"middle\" transform=\"rotate(-90 14 {})\">{}</text>",
        n(LEFT),
        n(baseline),
        n(WIDTH - RIGHT),
        n(baseline),
        n(LEFT + PLOT_WIDTH / 2.0),
        n(frame.height - 6.0),
        escape(x_label),
        n(TOP + frame.plot_height() / 2.0),
        n(TOP + frame.plot_height() / 2.0),
        escape(y_label)
    );
}

/// A vertical mark in the plot, with a short label at the top.
pub(crate) struct Marker {
    /// Where, in the units of the x axis.
    pub x: f64,
    /// The label at the top.
    pub label: String,
    /// The class of the mark (`marker`, `marker-p50`, ...).
    pub class: &'static str,
}

fn markers(out: &mut String, frame: &Frame, marks: &[Marker]) {
    for mark in marks {
        if mark.x < frame.x_min || mark.x > frame.x_max {
            continue;
        }
        let x = frame.x(mark.x);
        let _ = write!(
            out,
            "<line class=\"{}\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/><text class=\"marker-label\" x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
            mark.class,
            n(x),
            n(TOP),
            n(x),
            n(frame.y(0.0)),
            n(x),
            n(TOP - 6.0),
            escape(&mark.label)
        );
    }
}

/// The legend: a swatch and a name for each series, along the top right.
fn legend(out: &mut String, names: &[(&str, &str)]) {
    let mut x = WIDTH - RIGHT;
    for (name, class) in names.iter().rev() {
        let width = 22.0 + name.len() as f64 * 6.2;
        x -= width;
        let _ = write!(
            out,
            "<rect class=\"swatch {class}\" x=\"{}\" y=\"4\" width=\"10\" height=\"10\"/><text class=\"legend\" x=\"{}\" y=\"13\">{}</text>",
            n(x),
            n(x + 14.0),
            escape(name)
        );
    }
}

/// One line of a line chart. A point that is `None` breaks the line: the series has no value there (nothing arrived).
pub(crate) struct Line<'a> {
    /// The name in the legend.
    pub name: &'a str,
    /// The class of the line.
    pub class: &'a str,
    /// The points, in the units of the axes.
    pub points: Vec<Option<(f64, f64)>>,
}

/// What a line chart shows.
pub(crate) struct LineChart<'a> {
    /// The title of the picture.
    pub title: &'a str,
    /// What the picture says, for a screen reader.
    pub description: String,
    /// The end of the x axis, in seconds.
    pub x_max: f64,
    /// The name of the y axis.
    pub y_label: &'a str,
    /// Writes a value of the y axis.
    pub y_format: &'a dyn Fn(f64) -> String,
    /// The lines.
    pub lines: Vec<Line<'a>>,
    /// The marks (events of the timeline).
    pub marks: &'a [Marker],
}

/// A chart with lines over time.
pub(crate) fn line_chart(chart: &LineChart<'_>) -> String {
    let max = chart
        .lines
        .iter()
        .flat_map(|line| line.points.iter().flatten().map(|point| point.1))
        .fold(0.0_f64, f64::max);
    let (y_max, _) = nice_top(max);
    let frame = Frame {
        height: 240.0,
        x_min: 0.0,
        x_max: chart.x_max.max(f64::EPSILON),
        y_max,
    };
    let mut out = String::new();
    open(&mut out, frame.height, chart.title, &chart.description);
    axes(
        &mut out,
        &frame,
        "seconds from the start of the measured phase",
        chart.y_label,
        chart.y_format,
        &|v| tick(v),
    );
    markers(&mut out, &frame, chart.marks);
    for line in &chart.lines {
        let mut path = String::new();
        let mut open_segment = false;
        let mut singles = String::new();
        let points = &line.points;
        for (index, point) in points.iter().enumerate() {
            match point {
                Some((x, y)) => {
                    let command = if open_segment { 'L' } else { 'M' };
                    let _ = write!(path, "{command}{} {}", n(frame.x(*x)), n(frame.y(*y)));
                    open_segment = true;
                    let neighbours = [index.checked_sub(1), Some(index + 1)]
                        .into_iter()
                        .flatten()
                        .any(|other| points.get(other).is_some_and(Option::is_some));
                    if !neighbours {
                        let _ = write!(
                            singles,
                            "<circle class=\"dot {}\" cx=\"{}\" cy=\"{}\" r=\"2.5\"/>",
                            line.class,
                            n(frame.x(*x)),
                            n(frame.y(*y))
                        );
                    }
                }
                None => open_segment = false,
            }
        }
        if !path.is_empty() {
            let _ = write!(out, "<path class=\"line {}\" d=\"{path}\"/>", line.class);
        }
        out.push_str(&singles);
    }
    let names: Vec<(&str, &str)> = chart.lines.iter().map(|l| (l.name, l.class)).collect();
    legend(&mut out, &names);
    out.push_str("</svg>");
    out
}

/// One bar: from `x0` to `x1` on the x axis, `value` high.
pub(crate) struct Bar {
    /// The left edge.
    pub x0: f64,
    /// The right edge.
    pub x1: f64,
    /// The height.
    pub value: f64,
}

/// What a bar chart shows.
pub(crate) struct BarChart<'a> {
    /// The title of the picture.
    pub title: &'a str,
    /// What the picture says, for a screen reader.
    pub description: String,
    /// The class of the bars.
    pub class: &'a str,
    /// The start and the end of the x axis.
    pub x_range: (f64, f64),
    /// The name of the x axis.
    pub x_label: &'a str,
    /// Writes a value of the x axis.
    pub x_format: &'a dyn Fn(f64) -> String,
    /// The name of the y axis.
    pub y_label: &'a str,
    /// Writes a value of the y axis.
    pub y_format: &'a dyn Fn(f64) -> String,
    /// The bars.
    pub bars: Vec<Bar>,
    /// The marks.
    pub marks: &'a [Marker],
    /// The least height of the y axis (a share cannot be above 100 %, so its axis can be fixed).
    pub y_floor: f64,
}

/// A chart with bars.
pub(crate) fn bar_chart(chart: &BarChart<'_>) -> String {
    let max = chart
        .bars
        .iter()
        .map(|bar| bar.value)
        .fold(chart.y_floor, f64::max);
    let (y_max, _) = nice_top(max);
    let frame = Frame {
        height: 240.0,
        x_min: chart.x_range.0,
        x_max: chart.x_range.1.max(chart.x_range.0 + f64::EPSILON),
        y_max,
    };
    let mut out = String::new();
    open(&mut out, frame.height, chart.title, &chart.description);
    axes(
        &mut out,
        &frame,
        chart.x_label,
        chart.y_label,
        chart.y_format,
        chart.x_format,
    );
    for bar in &chart.bars {
        if bar.value <= 0.0 {
            continue;
        }
        let (x0, x1) = (frame.x(bar.x0), frame.x(bar.x1));
        let top = frame.y(bar.value);
        let _ = write!(
            out,
            "<rect class=\"bar {}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
            chart.class,
            n(x0),
            n(top),
            n((x1 - x0 - 1.0).max(1.0)),
            n(frame.y(0.0) - top)
        );
    }
    markers(&mut out, &frame, chart.marks);
    out.push_str("</svg>");
    out
}

/// What an interval plot shows: the value of every run of a set as a dot, the interval as a bar and the mean as a tick.
pub(crate) struct IntervalPlot<'a> {
    /// The title of the picture.
    pub title: &'a str,
    /// What the picture says, for a screen reader.
    pub description: String,
    /// The value of each run.
    pub values: &'a [f64],
    /// The mean (or the median) the interval is around.
    pub estimate: f64,
    /// The ends of the interval.
    pub interval: (f64, f64),
    /// Writes a value of the axis.
    pub format: &'a dyn Fn(f64) -> String,
}

/// A plot of the runs of a set against the interval of their mean.
pub(crate) fn interval_plot(plot: &IntervalPlot<'_>) -> String {
    let lowest = plot.values.iter().copied().fold(plot.interval.0, f64::min);
    let highest = plot.values.iter().copied().fold(plot.interval.1, f64::max);
    let pad = ((highest - lowest) * 0.08).max(highest.abs().max(1e-9) * 1e-6);
    let frame = Frame {
        height: 96.0,
        x_min: lowest - pad,
        x_max: highest + pad,
        y_max: 1.0,
    };
    let mut out = String::new();
    open(&mut out, frame.height, plot.title, &plot.description);
    let baseline = frame.height - 28.0;
    let middle = baseline - 26.0;
    let step = nice_step(frame.x_max - frame.x_min, 5.0);
    let mut value = (frame.x_min / step).ceil() * step;
    let _ = write!(
        out,
        "<line class=\"axis\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>",
        n(LEFT),
        n(baseline),
        n(WIDTH - RIGHT),
        n(baseline)
    );
    while value <= frame.x_max + step * 1e-9 {
        let x = frame.x(value);
        let _ = write!(
            out,
            "<line class=\"axis\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/><text class=\"tick\" x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
            n(x),
            n(baseline),
            n(x),
            n(baseline + 4.0),
            n(x),
            n(baseline + 16.0),
            escape(&(plot.format)(value))
        );
        value += step;
    }
    let _ = write!(
        out,
        "<rect class=\"interval\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"12\"/><line class=\"estimate\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"/>",
        n(frame.x(plot.interval.0)),
        n(middle - 6.0),
        n((frame.x(plot.interval.1) - frame.x(plot.interval.0)).max(1.0)),
        n(frame.x(plot.estimate)),
        n(middle - 12.0),
        n(frame.x(plot.estimate)),
        n(middle + 12.0)
    );
    for value in plot.values {
        let _ = write!(
            out,
            "<circle class=\"run\" cx=\"{}\" cy=\"{}\" r=\"3.5\"/>",
            n(frame.x(*value)),
            n(middle)
        );
    }
    out.push_str("</svg>");
    out
}

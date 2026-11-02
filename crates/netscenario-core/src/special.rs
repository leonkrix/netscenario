//! The two distribution functions that the expanded percentile interval needs (D-074): the upper tail of the normal
//! distribution and the upper quantile of Student's t. Written out here, from published algorithms, so that the crate needs no
//! statistics dependency for two functions.
//!
//! - `ln_gamma` is the Lanczos approximation (g = 7, nine terms), good to about 1e-15 for the arguments used here.
//! - The regularized incomplete beta function `I_x(a, b)` and the regularized upper incomplete gamma function `Q(a, x)` are
//!   summed as a series or a continued fraction (Lentz's method), the way the standard numerical texts give them.
//! - The upper tail of the normal distribution is `Q(1/2, z^2 / 2) / 2`; the upper tail of the t distribution with `nu`
//!   degrees of freedom is `I_{nu/(nu + t^2)}(nu / 2, 1/2) / 2`.
//! - The quantile of t is found by bisection on the tail, which is monotone, to a relative width of 1e-13.
//!
//! These use `exp`, `ln` and `powf` of the standard library, which the maths library of a machine may round differently in the
//! last bit. The results are used to choose an index among thousands, so a difference of that size can change the index only if
//! the exact value lies within 1e-12 of the middle between two indices; the module of the bootstrap says so.

#![allow(clippy::many_single_char_names)]

const EPS: f64 = 1e-15;
const TINY: f64 = 1e-300;

/// `ln(Gamma(x))` for `x > 0`.
fn ln_gamma(x: f64) -> f64 {
    const COEFFICIENTS: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // The reflection formula: Gamma(x) Gamma(1 - x) = pi / sin(pi x)
        return (std::f64::consts::PI / (std::f64::consts::PI * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let t = x + 7.5;
    let mut sum = COEFFICIENTS[0];
    for (index, coefficient) in COEFFICIENTS.iter().enumerate().skip(1) {
        // The index is below 9
        #[allow(clippy::cast_precision_loss)]
        let step = index as f64;
        sum += coefficient / (x + step);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + sum.ln()
}

/// The continued fraction of the incomplete beta function (Lentz).
fn beta_fraction(a: f64, b: f64, x: f64) -> f64 {
    let (qab, qap, qam) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < TINY {
        d = TINY;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=500_u32 {
        let m = f64::from(m);
        let m2 = 2.0 * m;
        let even = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + even * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + even / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        h *= d * c;
        let odd = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + odd * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + odd / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < EPS {
            break;
        }
    }
    h
}

/// The regularized incomplete beta function `I_x(a, b)` for `0 <= x <= 1`.
fn incomplete_beta(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let front =
        (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        front * beta_fraction(a, b, x) / a
    } else {
        1.0 - front * beta_fraction(b, a, 1.0 - x) / b
    }
}

/// The regularized upper incomplete gamma function `Q(a, x)` for `x >= 0`.
fn upper_gamma(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    let front = (-x + a * x.ln() - ln_gamma(a)).exp();
    if x < a + 1.0 {
        // The series for the lower function, P(a, x)
        let mut term = 1.0 / a;
        let mut sum = term;
        let mut ap = a;
        for _ in 0..1_000 {
            ap += 1.0;
            term *= x / ap;
            sum += term;
            if term.abs() < sum.abs() * EPS {
                break;
            }
        }
        1.0 - sum * front
    } else {
        // The continued fraction for Q(a, x)
        let mut b = x + 1.0 - a;
        let mut c = 1.0 / TINY;
        let mut d = 1.0 / b;
        let mut h = d;
        for i in 1..=1_000_u32 {
            let i = f64::from(i);
            let an = -i * (i - a);
            b += 2.0;
            d = an * d + b;
            if d.abs() < TINY {
                d = TINY;
            }
            c = b + an / c;
            if c.abs() < TINY {
                c = TINY;
            }
            d = 1.0 / d;
            let delta = d * c;
            h *= delta;
            if (delta - 1.0).abs() < EPS {
                break;
            }
        }
        front * h
    }
}

/// The probability that a standard normal number is above `z`, for `z >= 0`.
#[must_use]
pub(crate) fn normal_upper_tail(z: f64) -> f64 {
    0.5 * upper_gamma(0.5, z * z / 2.0)
}

/// The probability that a t number with `nu` degrees of freedom is above `t`, for `t >= 0`.
fn t_upper_tail(t: f64, nu: f64) -> f64 {
    0.5 * incomplete_beta(nu / 2.0, 0.5, nu / (nu + t * t))
}

/// The number `t >= 0` that a t number with `nu` degrees of freedom exceeds with the probability `tail`, for `0 < tail <=
/// 0.5`: the critical value of a two-sided interval of level `1 - 2 tail`.
#[must_use]
pub(crate) fn t_upper_quantile(tail: f64, nu: f64) -> f64 {
    let (mut low, mut high) = (0.0_f64, 1.0_f64);
    // Widen the upper end until it lies beyond the quantile
    while t_upper_tail(high, nu) > tail && high < 1e15 {
        high *= 2.0;
    }
    for _ in 0..200 {
        let middle = f64::midpoint(low, high);
        if t_upper_tail(middle, nu) > tail {
            low = middle;
        } else {
            high = middle;
        }
        if high - low <= 1e-13 * high {
            break;
        }
    }
    f64::midpoint(low, high)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(found: f64, expected: f64, relative: f64) {
        assert!(
            (found - expected).abs() <= relative * expected.abs(),
            "{found} against {expected}"
        );
    }

    /// Values from the published tables of the normal distribution.
    #[test]
    fn the_upper_tail_of_the_normal_distribution_matches_the_tables() {
        close(normal_upper_tail(0.0), 0.5, 1e-12);
        close(normal_upper_tail(1.0), 0.158_655_25, 1e-7);
        close(normal_upper_tail(1.96), 0.024_997_9, 1e-5);
        close(normal_upper_tail(3.0), 0.001_349_90, 1e-5);
        close(normal_upper_tail(5.0), 2.866_5e-7, 1e-4);
    }

    /// Critical values of the t distribution from the published tables, `t(0.975; nu)` and a few others.
    #[test]
    fn the_quantiles_of_t_match_the_tables() {
        for (tail, nu, expected) in [
            (0.025, 1.0, 12.706_204_7),
            (0.025, 2.0, 4.302_652_7),
            (0.025, 4.0, 2.776_445_1),
            (0.025, 9.0, 2.262_157_2),
            (0.025, 29.0, 2.045_229_6),
            (0.025, 99.0, 1.984_216_9),
            (0.05, 1.0, 6.313_751_5),
            (0.005, 9.0, 3.249_835_5),
        ] {
            close(t_upper_quantile(tail, nu), expected, 1e-6);
        }
    }

    /// Canary for the comparison above: a wrong degree of freedom does not give the table value.
    #[test]
    fn the_tables_would_notice_a_wrong_degree_of_freedom() {
        assert!((t_upper_quantile(0.025, 5.0) - 2.776_445_1).abs() > 0.1);
    }

    /// For many degrees of freedom the t distribution is the normal one.
    #[test]
    fn the_t_distribution_approaches_the_normal_one() {
        close(t_upper_quantile(0.025, 1.0e6), 1.959_964, 1e-5);
    }
}

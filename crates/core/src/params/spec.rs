//! Per-field specification (range, step, unit, description) and the visitor used for
//! validation, clamping, rounding, path access and schema annotation.
//!
//! Every scalar slider is declared exactly once, in a `scalar_group!` invocation. The spec
//! drives clamping (doc 02 §4 "hard-clamped for every writer"), rounding to the field's step,
//! `ParamPath` access and the `minimum`/`maximum`/`multipleOf` annotations in the JSON Schema.

/// Static description of one scalar parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldSpec {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    /// Stored precision: values are rounded to a multiple of `step` at commit (doc 02 §4).
    pub step: f32,
    pub default: f32,
    pub unit: &'static str,
    pub doc: &'static str,
}

impl FieldSpec {
    /// Clamp to the range and round to the step. `-0.0` is normalised to `0.0` so that the
    /// canonical hash (doc 02 §11) does not distinguish them.
    pub fn normalize(&self, v: f32) -> f32 {
        let clamped = v.clamp(self.min, self.max);
        let rounded = round_to_step(clamped, self.step).clamp(self.min, self.max);
        rounded + 0.0
    }
}

/// Round `v` to the nearest multiple of `step`, computed in f64 so that values which are
/// already on the grid survive a round trip unchanged.
pub fn round_to_step(v: f32, step: f32) -> f32 {
    if step <= 0.0 {
        return v;
    }
    // Work in integer units of 10^-d, where d is the number of decimals of the (f32) step, so
    // that e.g. 30 × 0.01 becomes exactly 30 / 100 and lands on the f32 nearest to 0.3.
    let decimals = decimals_of(step);
    let p = 10f64.powi(decimals);
    let step_units = (step as f64 * p).round();
    let n = (v as f64 * p / step_units).round();
    (n * step_units / p) as f32
}

fn decimals_of(step: f32) -> i32 {
    let mut d = 0;
    let mut s = step as f64;
    while d < 9 && (s - s.round()).abs() > 1e-5 * s.abs() {
        s *= 10.0;
        d += 1;
    }
    d
}

pub(crate) fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}.{name}")
    }
}

/// Visitor over the scalar parameters of a params struct, addressed by dotted path.
pub trait Scalars {
    fn visit(&self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, f32));
    fn visit_mut(&mut self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, &mut f32));
}

/// Declares a struct whose fields are all scalar `f32` sliders with a spec each.
///
/// ```ignore
/// scalar_group! {
///     /// Global light.
///     pub struct LightParams {
///         /// Exposure.
///         exposure: 0.0, [-5.0, 5.0], step 0.01, "EV";
///     }
/// }
/// ```
macro_rules! scalar_group {
    (
        $(#[$sm:meta])*
        pub struct $name:ident {
            $(
                $(#[doc = $doc:literal])*
                $field:ident : $default:expr, [$min:expr, $max:expr], step $step:expr, $unit:literal;
            )*
        }
    ) => {
        $(#[$sm])*
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        #[serde(default)]
        pub struct $name {
            $(
                $(#[doc = $doc])*
                pub $field: f32,
            )*
        }

        impl Default for $name {
            fn default() -> Self {
                Self { $($field: $default,)* }
            }
        }

        impl $name {
            pub const SPECS: &'static [$crate::params::FieldSpec] = &[
                $(
                    $crate::params::FieldSpec {
                        name: stringify!($field),
                        min: $min,
                        max: $max,
                        step: $step,
                        default: $default,
                        unit: $unit,
                        doc: concat!($($doc),*),
                    },
                )*
            ];
        }

        impl $crate::params::Scalars for $name {
            #[allow(unused_assignments, unused_variables, unused_mut)]
            fn visit(&self, prefix: &str, f: &mut dyn FnMut(&str, &$crate::params::FieldSpec, f32)) {
                let mut i = 0usize;
                $(
                    f(&$crate::params::spec::join(prefix, stringify!($field)), &Self::SPECS[i], self.$field);
                    i += 1;
                )*
            }

            #[allow(unused_assignments, unused_variables, unused_mut)]
            fn visit_mut(
                &mut self,
                prefix: &str,
                f: &mut dyn FnMut(&str, &$crate::params::FieldSpec, &mut f32),
            ) {
                let mut i = 0usize;
                $(
                    f(&$crate::params::spec::join(prefix, stringify!($field)), &Self::SPECS[i], &mut self.$field);
                    i += 1;
                )*
            }
        }
    };
}

/// Declares a struct whose fields are all [`Scalars`] groups, nested under the field name.
macro_rules! nested_group {
    (
        $(#[$sm:meta])*
        pub struct $name:ident {
            $(
                $(#[doc = $doc:literal])*
                $field:ident : $ty:ty,
            )*
        }
    ) => {
        $(#[$sm])*
        #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        #[serde(default)]
        pub struct $name {
            $(
                $(#[doc = $doc])*
                pub $field: $ty,
            )*
        }

        impl $crate::params::Scalars for $name {
            fn visit(&self, prefix: &str, f: &mut dyn FnMut(&str, &$crate::params::FieldSpec, f32)) {
                $( self.$field.visit(&$crate::params::spec::join(prefix, stringify!($field)), f); )*
            }

            fn visit_mut(
                &mut self,
                prefix: &str,
                f: &mut dyn FnMut(&str, &$crate::params::FieldSpec, &mut f32),
            ) {
                $( self.$field.visit_mut(&$crate::params::spec::join(prefix, stringify!($field)), f); )*
            }
        }
    };
}

pub(crate) use nested_group;
pub(crate) use scalar_group;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_is_idempotent_and_exact_on_grid() {
        for &(v, step, want) in &[
            (0.304f32, 0.01f32, 0.3f32),
            (0.305, 0.01, 0.31),
            (-0.004, 0.01, 0.0),
            (12.6, 1.0, 13.0),
            (0.1234567, 0.000001, 0.123457),
        ] {
            let r = round_to_step(v, step);
            assert_eq!(r, want, "round({v}, {step})");
            assert_eq!(round_to_step(r, step), r, "idempotent");
        }
    }

    #[test]
    fn normalize_clamps_and_kills_negative_zero() {
        let s = FieldSpec {
            name: "x",
            min: -5.0,
            max: 5.0,
            step: 0.01,
            default: 0.0,
            unit: "EV",
            doc: "",
        };
        assert_eq!(s.normalize(9.0), 5.0);
        assert_eq!(s.normalize(-0.001).to_bits(), 0.0f32.to_bits());
    }
}

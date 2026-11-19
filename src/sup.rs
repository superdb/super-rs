//! Formatting of values in SUP, the text format of SuperDB.
//!
//! The output follows the conventions of `super -s` (e.g., values of
//! fusion types are shown as values of their subtypes) but isn't guaranteed
//! to match it character for character.

use std::collections::HashSet;

use crate::types::{format_field_name, format_type_name, Primitive, Type};
use crate::value::{quote_string, Value};

/// Formats values as a stream of SUP text, emitting a type declaration
/// (`type name=type`) before the first value that refers to each named
/// type.
#[derive(Default)]
pub struct Formatter {
    declared: HashSet<String>,
}

impl Formatter {
    /// Returns a new formatter.
    pub fn new() -> Formatter {
        Formatter::default()
    }

    /// Returns the SUP text of `v`, preceded by declarations of the named
    /// types it refers to that weren't declared by earlier calls, each on a
    /// line of its own.
    pub fn format(&mut self, v: &Value) -> String {
        let mut s = String::new();
        let mut decls = Vec::new();
        crate::types::declare(&v.typ(), &mut self.declared, &mut decls);
        collect_type_value_decls(v, &mut self.declared, &mut decls);
        for d in decls {
            s.push_str(&d);
            s.push('\n');
        }
        fmt_value(&mut s, v, Deco::Yes);
        s
    }
}

fn collect_type_value_decls(v: &Value, seen: &mut HashSet<String>, out: &mut Vec<String>) {
    match v {
        Value::Type(t) => crate::types::declare(t, seen, out),
        Value::Record(fields) => fields
            .iter()
            .for_each(|(_, v)| collect_type_value_decls(v, seen, out)),
        Value::Array(_, vals) | Value::Set(_, vals) => vals
            .iter()
            .for_each(|v| collect_type_value_decls(v, seen, out)),
        Value::Map(_, _, entries) => entries.iter().for_each(|(k, v)| {
            collect_type_value_decls(k, seen, out);
            collect_type_value_decls(v, seen, out)
        }),
        Value::Union(_, v)
        | Value::Error(v)
        | Value::Named(_, v)
        | Value::Option(_, Some(v))
        | Value::Fusion(_, v, _) => collect_type_value_decls(v, seen, out),
        _ => {}
    }
}

/// Returns the SUP text of `v` without type declarations.
pub fn format_value(v: &Value) -> String {
    let mut s = String::new();
    fmt_value(&mut s, v, Deco::Yes);
    s
}

#[derive(Clone, Copy, PartialEq)]
enum Deco {
    /// Decorate the value if its type isn't implied by its syntax.
    Yes,
    /// The type of the value is implied by its context.
    No,
}

/// Returns true if the syntax of a value of primitive type `p` implies its
/// type.
fn implied(p: Primitive) -> bool {
    use Primitive::*;
    matches!(
        p,
        Int64
            | Duration
            | Timestamp
            | Float64
            | Bool
            | Bytes
            | String
            | Ip
            | Net
            | Uuid
            | Type
            | Null
            | Missing
            | None
    )
}

fn decorate(s: &mut String, t: &Type) {
    s.push_str("::");
    if matches!(t, Type::Union(_)) {
        s.push('(');
        s.push_str(&t.to_string());
        s.push(')');
    } else {
        s.push_str(&t.to_string());
    }
}

/// Returns the type implied by the values of a container, which is `none`
/// for no values and otherwise the union of their types.
fn implied_elem_type(vals: &mut dyn Iterator<Item = &Value>) -> Type {
    let types: Vec<Type> = vals
        .map(|v| match v {
            Value::Union(_, inner) => inner.typ(),
            v => v.typ(),
        })
        .collect();
    if types.is_empty() {
        return Type::NONE;
    }
    Type::union(types).unwrap_or(Type::NONE)
}

fn fmt_value(s: &mut String, v: &Value, deco: Deco) {
    match v {
        Value::Uint8(x) => prim(s, &x.to_string(), Primitive::Uint8, deco),
        Value::Uint16(x) => prim(s, &x.to_string(), Primitive::Uint16, deco),
        Value::Uint32(x) => prim(s, &x.to_string(), Primitive::Uint32, deco),
        Value::Uint64(x) => prim(s, &x.to_string(), Primitive::Uint64, deco),
        Value::Int8(x) => prim(s, &x.to_string(), Primitive::Int8, deco),
        Value::Int16(x) => prim(s, &x.to_string(), Primitive::Int16, deco),
        Value::Int32(x) => prim(s, &x.to_string(), Primitive::Int32, deco),
        Value::Int64(x) => s.push_str(&x.to_string()),
        Value::Duration(x) => s.push_str(&format_duration(*x)),
        Value::Timestamp(x) => s.push_str(&format_timestamp(*x)),
        Value::Float16(x) => prim(
            s,
            &format_float(f64::from(*x), true),
            Primitive::Float16,
            deco,
        ),
        Value::Float32(x) => prim(
            s,
            &format_float(f64::from(*x), true),
            Primitive::Float32,
            deco,
        ),
        Value::Float64(x) => s.push_str(&format_float(*x, false)),
        Value::Bool(x) => s.push_str(if *x { "true" } else { "false" }),
        Value::Bytes(b) => {
            s.push_str("0x");
            for byte in b {
                s.push_str(&format!("{byte:02x}"));
            }
        }
        Value::String(x) => quote_string(s, x),
        Value::Ip(ip) => s.push_str(&ip.to_string()),
        Value::Net(ip, n) => s.push_str(&format!("{ip}/{n}")),
        Value::Uuid(u) => {
            for (i, b) in u.iter().enumerate() {
                if matches!(i, 4 | 6 | 8 | 10) {
                    s.push('-');
                }
                s.push_str(&format!("{b:02x}"));
            }
        }
        Value::Type(t) => {
            s.push('<');
            s.push_str(&t.to_string());
            s.push('>');
        }
        Value::Null => s.push_str("null"),
        Value::Missing => s.push_str("missing"),
        Value::None => s.push_str("none"),
        Value::Record(fields) => {
            s.push('{');
            for (i, (name, v)) in fields.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                format_field_name(s, name);
                match v {
                    Value::Option(t, opt) => {
                        s.push_str("?:");
                        match opt {
                            Some(v) => fmt_value(s, v, Deco::Yes),
                            None => {
                                s.push_str("none");
                                decorate(s, t);
                            }
                        }
                    }
                    v => {
                        s.push(':');
                        fmt_value(s, v, Deco::Yes);
                    }
                }
            }
            s.push('}');
        }
        Value::Array(elem, vals) => {
            container(s, "[", "]", vals.iter(), elem);
            if deco == Deco::Yes && implied_elem_type(&mut vals.iter()) != *elem {
                decorate(s, &Type::array(elem.clone()));
            }
        }
        Value::Set(elem, vals) => {
            container(s, "set[", "]", vals.iter(), elem);
            if deco == Deco::Yes && implied_elem_type(&mut vals.iter()) != *elem {
                decorate(s, &Type::set(elem.clone()));
            }
        }
        Value::Map(k, val, entries) => {
            s.push_str("map{");
            for (i, (key, v)) in entries.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                elem(s, key, k);
                if matches!(key.under(), Value::Ip(std::net::IpAddr::V6(_))) {
                    s.push(' ');
                }
                s.push(':');
                elem(s, v, val);
            }
            s.push('}');
            let ki = implied_elem_type(&mut entries.iter().map(|(k, _)| k));
            let vi = implied_elem_type(&mut entries.iter().map(|(_, v)| v));
            if deco == Deco::Yes && (ki != *k || vi != *val) {
                decorate(s, &Type::map(k.clone(), val.clone()));
            }
        }
        Value::Union(t, v) => {
            fmt_value(s, v, Deco::Yes);
            if deco == Deco::Yes {
                decorate(s, t);
            }
        }
        Value::Enum(symbols, k) => {
            quote_string(s, &symbols[*k as usize]);
            if deco == Deco::Yes {
                decorate(s, &Type::Enum(symbols.clone()));
            }
        }
        Value::Error(v) => {
            s.push_str("error(");
            fmt_value(s, v, Deco::Yes);
            s.push(')');
        }
        Value::Named(n, v) => {
            fmt_value(s, v, Deco::No);
            if deco == Deco::Yes {
                s.push_str("::");
                format_type_name(s, n.name());
            }
        }
        Value::Option(t, v) => match v {
            Some(v) => {
                s.push_str("some(");
                fmt_value(s, v, Deco::Yes);
                s.push(')');
            }
            None => {
                s.push_str("none");
                decorate(s, t);
            }
        },
        Value::Fusion(..) => fmt_value(s, &v.defuse(), deco),
        Value::Reserved(t, b) => {
            s.push_str("0x");
            for byte in b {
                s.push_str(&format!("{byte:02x}"));
            }
            decorate(s, t);
        }
    }
}

fn container<'a>(
    s: &mut String,
    open: &str,
    close: &str,
    vals: impl Iterator<Item = &'a Value>,
    t: &Type,
) {
    s.push_str(open);
    for (i, v) in vals.enumerate() {
        if i > 0 {
            s.push(',');
        }
        elem(s, v, t);
    }
    s.push_str(close);
}

/// Formats an element of a container whose element type is `t`.  The union
/// type of a union element is implied by the container's type.
fn elem(s: &mut String, v: &Value, _t: &Type) {
    match v {
        Value::Union(_, inner) => fmt_value(s, inner, Deco::Yes),
        v => fmt_value(s, v, Deco::Yes),
    }
}

fn prim(s: &mut String, text: &str, p: Primitive, deco: Deco) {
    s.push_str(text);
    if deco == Deco::Yes && !implied(p) {
        s.push_str("::");
        s.push_str(p.name());
    }
}

/// Formats a float as SUP does: an integral value as an integer followed by
/// a period and otherwise as Go's `strconv.FormatFloat(f, 'g', -1, bits)`.
pub(crate) fn format_float(v: f64, single: bool) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "+Inf".into()
        } else {
            "-Inf".into()
        };
    }
    if v == v.trunc() && v.abs() < 9.2e18 {
        return format!("{}.", v as i64);
    }
    // The shortest digits that round-trip and the decimal exponent.
    let e = if single {
        format!("{:e}", v as f32)
    } else {
        format!("{v:e}")
    };
    let (mantissa, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let neg = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if !(-4..6).contains(&exp) {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exp.abs()));
    } else if exp < 0 {
        out.push_str("0.");
        for _ in 0..(-exp - 1) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let point = exp as usize + 1;
        if digits.len() <= point {
            out.push_str(&digits);
            for _ in digits.len()..point {
                out.push('0');
            }
        } else {
            out.push_str(&digits[..point]);
            out.push('.');
            out.push_str(&digits[point..]);
        }
    }
    out
}

/// Formats a duration in nanoseconds as Go's `time.Duration.String`.
pub(crate) fn format_duration(d: i64) -> String {
    let neg = d < 0;
    let mut u = d.unsigned_abs();
    let mut out = String::new();
    if u < 1_000_000_000 {
        let (unit, prec) = match u {
            0 => return "0s".into(),
            n if n < 1_000 => ("ns", 0),
            n if n < 1_000_000 => ("us", 3),
            _ => ("ms", 6),
        };
        out.push_str(&frac_int(u, prec));
        out.push_str(unit);
    } else {
        let secs = frac_int(u % 60_000_000_000, 9);
        u /= 60_000_000_000;
        let mut s = format!("{secs}s");
        if u > 0 {
            s = format!("{}m{s}", u % 60);
            u /= 60;
            if u > 0 {
                s = format!("{u}h{s}");
            }
        }
        out = s;
    }
    if neg {
        out.insert(0, '-');
    }
    out
}

/// Formats `v` with the last `prec` digits as a fraction without trailing
/// zeros.
fn frac_int(v: u64, prec: u32) -> String {
    let p = 10u64.pow(prec);
    let (int, frac) = (v / p, v % p);
    if frac == 0 {
        return int.to_string();
    }
    let f = format!("{:0width$}", frac, width = prec as usize);
    format!("{int}.{}", f.trim_end_matches('0'))
}

/// Formats a timestamp in nanoseconds since the Unix epoch in RFC 3339 with
/// nanoseconds, as Go's `time.RFC3339Nano` in UTC.
pub(crate) fn format_timestamp(ts: i64) -> String {
    let secs = ts.div_euclid(1_000_000_000);
    let nanos = ts.rem_euclid(1_000_000_000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let mut s = format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    );
    if nanos != 0 {
        let f = format!("{nanos:09}");
        s.push('.');
        s.push_str(f.trim_end_matches('0'));
    }
    s.push('Z');
    s
}

/// Returns the civil date of days since 1970-01-01 (from Howard Hinnant's
/// date algorithms).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats() {
        assert_eq!(format_float(1.0, false), "1.");
        assert_eq!(format_float(3.5, false), "3.5");
        assert_eq!(format_float(1234567.5, false), "1.2345675e+06");
        assert_eq!(format_float(123456.5, false), "123456.5");
        assert_eq!(format_float(0.0001, false), "0.0001");
        assert_eq!(format_float(0.00001, false), "1e-05");
        assert_eq!(format_float(1e100, false), "1e+100");
        assert_eq!(format_float(f64::from(0.1f32), true), "0.1");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(1), "1ns");
        assert_eq!(format_duration(1_500), "1.5us");
        assert_eq!(format_duration(3_723_000_000_000), "1h2m3s");
        assert_eq!(format_duration(90_500_000_000), "1m30.5s");
        assert_eq!(format_duration(-2_000_000), "-2ms");
        assert_eq!(format_duration(3_600_000_000_000), "1h0m0s");
    }

    #[test]
    fn timestamps() {
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_timestamp(1_709_296_496_789_000_000),
            "2024-03-01T12:34:56.789Z"
        );
        assert_eq!(format_timestamp(-1), "1969-12-31T23:59:59.999999999Z");
    }
}

//! Port of lib.ts: pure helpers. No I/O here, so the tests below can cover them (migrate_db takes the database, so the
//! tests give it one in memory).
//!
//! PORT: the TS code reads `any` input with optional chaining. Where malformed input (a null array element, a
//! non-array where an array belongs) would throw a TypeError in JS, these read it as missing instead.

use crate::util::{truthy, J};
use chrono::{DateTime, Datelike, Duration, Local, LocalResult, NaiveDateTime, TimeZone, Timelike};
use regex::Regex;
use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

// ---------- JS semantics the port keeps ----------

/// JS whitespace (`\s`, `trim()`). Rust's White_Space adds U+0085 and lacks U+FEFF.
fn js_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
/// JS `\s` as the inside of a regex class.
const WS: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";
fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}
fn js_trim(s: &str) -> &str {
    s.trim_matches(js_ws)
}
/// `s.length`: JS counts UTF-16 units.
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}
/// `s.slice(0, n)` in UTF-16 units. PORT: a cut through a surrogate pair drops that character (JS keeps half of it).
fn js_head(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > n {
            return &s[..i];
        }
    }
    s
}
/// `s.slice(n)` in UTF-16 units.
fn js_from(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units >= n {
            return &s[i..];
        }
        units += c.len_utf16();
    }
    ""
}
/// `s.slice(-n)` in UTF-16 units.
fn js_tail(s: &str, n: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices().rev() {
        units += c.len_utf16();
        if units > n {
            return &s[i + c.len_utf8()..];
        }
    }
    s
}
/// `String(v)`; None is undefined.
fn js_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.as_i64().map(|i| i.to_string()).unwrap_or_else(|| num_text(n.as_f64().unwrap_or(f64::NAN))),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().map(join_elem).collect::<Vec<_>>().join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}
/// An element as `Array.prototype.join` writes it: null and undefined are empty.
fn join_elem(v: &Value) -> String {
    if v.is_null() {
        String::new()
    } else {
        js_string(Some(v))
    }
}
/// A number as JS prints it, for the forms the app meets.
fn num_text(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        (if f > 0.0 { "Infinity" } else { "-Infinity" }).into()
    } else if f == 0.0 {
        "0".into()
    } else if f.fract() == 0.0 && f.abs() < 1e21 {
        format!("{f:.0}")
    } else {
        f.to_string()
    }
}
/// A JS number as JSON: integral values as integers, NaN and Infinity as null (JSON.stringify does the same).
fn num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9.0e15 {
        json!(f as i64)
    } else {
        serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
    }
}
/// `Number(s)`.
fn js_number(s: &str) -> f64 {
    let t = js_trim(s);
    if t.is_empty() {
        return 0.0;
    }
    if let Some(r) = t.get(..2).and_then(|p| match p {
        "0x" | "0X" => Some(16),
        "0o" | "0O" => Some(8),
        "0b" | "0B" => Some(2),
        _ => None,
    }) {
        let d = &t[2..];
        return if !d.is_empty() && d.chars().all(|c| c.is_digit(r)) {
            d.chars().fold(0.0, |n, c| n * r as f64 + c.to_digit(r).unwrap() as f64)
        } else {
            f64::NAN
        };
    }
    match t {
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ if t.bytes().all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b)) => t.parse().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}
/// `Number(v)`; None is undefined.
fn to_number(v: Option<&Value>) -> f64 {
    match v {
        None | Some(Value::Object(_)) => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => *b as u8 as f64,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => js_number(s),
        Some(Value::Array(_)) => js_number(&js_string(v)),
    }
}
/// JS truthiness of a number.
fn num_truthy(f: f64) -> bool {
    f != 0.0 && !f.is_nan()
}
/// The left side of `??`: None when undefined or null.
fn nn(v: Option<&Value>) -> Option<&Value> {
    v.filter(|x| !x.is_null())
}
/// `a?.b?.c`
fn at<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().try_fold(v, |v, k| v.get(*k))
}
/// The elements of an array field; none when it is missing or not an array.
fn arr(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}
/// `===` for JSON values. PORT: objects and arrays compare by content, not identity.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}
fn index_key(k: &str) -> Option<u32> {
    let canonical = k == "0" || (!k.is_empty() && !k.starts_with('0') && k.bytes().all(|b| b.is_ascii_digit()));
    k.parse::<u32>().ok().filter(|n| canonical && *n != u32::MAX)
}
/// The key order of a JS object: integer keys ascending first, then the others in insertion order.
fn js_order(m: Map<String, Value>) -> Map<String, Value> {
    if !m.keys().any(|k| index_key(k).is_some()) {
        return m;
    }
    let (mut ints, rest): (Vec<_>, Vec<_>) = m.into_iter().partition(|(k, _)| index_key(k).is_some());
    ints.sort_by_key(|(k, _)| index_key(k));
    ints.into_iter().chain(rest).collect()
}
/// `{ ...v }`
fn js_spread(v: &Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => js_order(m.clone()),
        Value::Array(a) => a.iter().enumerate().map(|(i, x)| (i.to_string(), x.clone())).collect(),
        Value::String(s) => s.chars().enumerate().map(|(i, c)| (i.to_string(), json!(c.to_string()))).collect(),
        _ => Map::new(),
    }
}
/// `Object.entries(v)`
fn js_entries(v: &Value) -> Vec<(String, Value)> {
    match v {
        Value::Object(_) | Value::Array(_) | Value::String(_) => js_spread(v).into_iter().collect(),
        _ => vec![],
    }
}
/// `JSON.stringify(v)`: every object in JS key order.
fn stringify(v: &Value) -> String {
    fn ordered(v: &Value) -> Value {
        match v {
            Value::Object(m) => Value::Object(js_order(m.iter().map(|(k, x)| (k.clone(), ordered(x))).collect())),
            Value::Array(a) => Value::Array(a.iter().map(ordered).collect()),
            _ => v.clone(),
        }
    }
    serde_json::to_string(&ordered(v)).expect("JSON")
}
/// Sets a field, or leaves it out where JS would set undefined (JSON.stringify drops it).
fn put(m: &mut Map<String, Value>, k: &str, v: Option<&Value>) {
    match v {
        Some(v) => {
            m.insert(k.into(), v.clone());
        }
        None => {
            m.shift_remove(k);
        }
    }
}
fn is_sep(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}
/// node's `path.basename` (posix). PORT: on Windows it also splits on backslashes, without node's drive-letter rules.
fn js_basename(p: &str) -> &str {
    let t = p.trim_end_matches(is_sep);
    match t.rfind(is_sep) {
        Some(i) => &t[i + 1..],
        None => t,
    }
}
/// node's `path.dirname` (posix). PORT: on Windows it also splits on backslashes, without node's drive-letter rules.
fn js_dirname(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    // node's own loop: the last separator before the last name, past trailing separators
    let b = p.as_bytes();
    let root = is_sep(b[0] as char);
    let mut end = None;
    let mut matched_sep = true;
    for i in (1..b.len()).rev() {
        if is_sep(b[i] as char) {
            if !matched_sep {
                end = Some(i);
                break;
            }
        } else {
            matched_sep = false;
        }
    }
    match end {
        None if root => p[..1].into(),
        None => ".".into(),
        Some(1) if root => p[..2].into(),
        Some(e) => p[..e].into(),
    }
}
/// A path's components after `path.resolve` (absolute against the current directory, `.` and `..` applied).
fn resolved(p: &str) -> Vec<String> {
    let abs = if p.starts_with(is_sep) {
        p.to_string()
    } else {
        format!("{}/{p}", std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default())
    };
    let mut out: Vec<String> = vec![];
    for part in abs.split(is_sep) {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            _ => out.push(if cfg!(windows) { part.to_lowercase() } else { part.to_string() }),
        }
    }
    out
}

// ---------- Unicode NFKD for slugify ----------

/// The characters NFKD (Unicode 15.1) turns into ASCII letters or digits, mapped to what is left of them once slugify
/// lowercases and dashes the rest (a combining mark is a dash). Every other non-ASCII character becomes a dash.
/// PORT: Rust's standard library has no Unicode normalization, so this table, generated with Python's unicodedata,
/// stands in for `normalize("NFKD")`.
#[rustfmt::skip]
const NFKD: &[(u32, &str)] = &[
    (0xAA, "a"), (0xB2, "2"), (0xB3, "3"), (0xB9, "1"), (0xBA, "o"), (0xBC, "1-4"), (0xBD, "1-2"), (0xBE, "3-4"),
    (0xE0, "a-"), (0xE1, "a-"), (0xE2, "a-"), (0xE3, "a-"), (0xE4, "a-"), (0xE5, "a-"), (0xE7, "c-"), (0xE8, "e-"),
    (0xE9, "e-"), (0xEA, "e-"), (0xEB, "e-"), (0xEC, "i-"), (0xED, "i-"), (0xEE, "i-"), (0xEF, "i-"), (0xF1, "n-"),
    (0xF2, "o-"), (0xF3, "o-"), (0xF4, "o-"), (0xF5, "o-"), (0xF6, "o-"), (0xF9, "u-"), (0xFA, "u-"), (0xFB, "u-"),
    (0xFC, "u-"), (0xFD, "y-"), (0xFF, "y-"), (0x101, "a-"), (0x103, "a-"), (0x105, "a-"), (0x107, "c-"),
    (0x109, "c-"), (0x10B, "c-"), (0x10D, "c-"), (0x10F, "d-"), (0x113, "e-"), (0x115, "e-"), (0x117, "e-"),
    (0x119, "e-"), (0x11B, "e-"), (0x11D, "g-"), (0x11F, "g-"), (0x121, "g-"), (0x123, "g-"), (0x125, "h-"),
    (0x129, "i-"), (0x12B, "i-"), (0x12D, "i-"), (0x12F, "i-"), (0x133, "ij"), (0x135, "j-"), (0x137, "k-"),
    (0x13A, "l-"), (0x13C, "l-"), (0x13E, "l-"), (0x140, "l-"), (0x144, "n-"), (0x146, "n-"), (0x148, "n-"),
    (0x149, "-n"), (0x14D, "o-"), (0x14F, "o-"), (0x151, "o-"), (0x155, "r-"), (0x157, "r-"), (0x159, "r-"),
    (0x15B, "s-"), (0x15D, "s-"), (0x15F, "s-"), (0x161, "s-"), (0x163, "t-"), (0x165, "t-"), (0x169, "u-"),
    (0x16B, "u-"), (0x16D, "u-"), (0x16F, "u-"), (0x171, "u-"), (0x173, "u-"), (0x175, "w-"), (0x177, "y-"),
    (0x17A, "z-"), (0x17C, "z-"), (0x17E, "z-"), (0x17F, "s"), (0x1A1, "o-"), (0x1B0, "u-"), (0x1C5, "-z-"),
    (0x1C6, "dz-"), (0x1C8, "-j"), (0x1C9, "lj"), (0x1CB, "-j"), (0x1CC, "nj"), (0x1CE, "a-"), (0x1D0, "i-"),
    (0x1D2, "o-"), (0x1D4, "u-"), (0x1D6, "u-"), (0x1D8, "u-"), (0x1DA, "u-"), (0x1DC, "u-"), (0x1DF, "a-"),
    (0x1E1, "a-"), (0x1E7, "g-"), (0x1E9, "k-"), (0x1EB, "o-"), (0x1ED, "o-"), (0x1F0, "j-"), (0x1F2, "-z"),
    (0x1F3, "dz"), (0x1F5, "g-"), (0x1F9, "n-"), (0x1FB, "a-"), (0x201, "a-"), (0x203, "a-"), (0x205, "e-"),
    (0x207, "e-"), (0x209, "i-"), (0x20B, "i-"), (0x20D, "o-"), (0x20F, "o-"), (0x211, "r-"), (0x213, "r-"),
    (0x215, "u-"), (0x217, "u-"), (0x219, "s-"), (0x21B, "t-"), (0x21F, "h-"), (0x227, "a-"), (0x229, "e-"),
    (0x22B, "o-"), (0x22D, "o-"), (0x22F, "o-"), (0x231, "o-"), (0x233, "y-"), (0x2B0, "h"), (0x2B2, "j"),
    (0x2B3, "r"), (0x2B7, "w"), (0x2B8, "y"), (0x2E1, "l"), (0x2E2, "s"), (0x2E3, "x"), (0x1D43, "a"), (0x1D47, "b"),
    (0x1D48, "d"), (0x1D49, "e"), (0x1D4D, "g"), (0x1D4F, "k"), (0x1D50, "m"), (0x1D52, "o"), (0x1D56, "p"),
    (0x1D57, "t"), (0x1D58, "u"), (0x1D5B, "v"), (0x1D62, "i"), (0x1D63, "r"), (0x1D64, "u"), (0x1D65, "v"),
    (0x1D9C, "c"), (0x1DA0, "f"), (0x1DBB, "z"), (0x1E01, "a-"), (0x1E03, "b-"), (0x1E05, "b-"), (0x1E07, "b-"),
    (0x1E09, "c-"), (0x1E0B, "d-"), (0x1E0D, "d-"), (0x1E0F, "d-"), (0x1E11, "d-"), (0x1E13, "d-"), (0x1E15, "e-"),
    (0x1E17, "e-"), (0x1E19, "e-"), (0x1E1B, "e-"), (0x1E1D, "e-"), (0x1E1F, "f-"), (0x1E21, "g-"), (0x1E23, "h-"),
    (0x1E25, "h-"), (0x1E27, "h-"), (0x1E29, "h-"), (0x1E2B, "h-"), (0x1E2D, "i-"), (0x1E2F, "i-"), (0x1E31, "k-"),
    (0x1E33, "k-"), (0x1E35, "k-"), (0x1E37, "l-"), (0x1E39, "l-"), (0x1E3B, "l-"), (0x1E3D, "l-"), (0x1E3F, "m-"),
    (0x1E41, "m-"), (0x1E43, "m-"), (0x1E45, "n-"), (0x1E47, "n-"), (0x1E49, "n-"), (0x1E4B, "n-"), (0x1E4D, "o-"),
    (0x1E4F, "o-"), (0x1E51, "o-"), (0x1E53, "o-"), (0x1E55, "p-"), (0x1E57, "p-"), (0x1E59, "r-"), (0x1E5B, "r-"),
    (0x1E5D, "r-"), (0x1E5F, "r-"), (0x1E61, "s-"), (0x1E63, "s-"), (0x1E65, "s-"), (0x1E67, "s-"), (0x1E69, "s-"),
    (0x1E6B, "t-"), (0x1E6D, "t-"), (0x1E6F, "t-"), (0x1E71, "t-"), (0x1E73, "u-"), (0x1E75, "u-"), (0x1E77, "u-"),
    (0x1E79, "u-"), (0x1E7B, "u-"), (0x1E7D, "v-"), (0x1E7F, "v-"), (0x1E81, "w-"), (0x1E83, "w-"), (0x1E85, "w-"),
    (0x1E87, "w-"), (0x1E89, "w-"), (0x1E8B, "x-"), (0x1E8D, "x-"), (0x1E8F, "y-"), (0x1E91, "z-"), (0x1E93, "z-"),
    (0x1E95, "z-"), (0x1E96, "h-"), (0x1E97, "t-"), (0x1E98, "w-"), (0x1E99, "y-"), (0x1E9A, "a-"), (0x1E9B, "s-"),
    (0x1EA1, "a-"), (0x1EA3, "a-"), (0x1EA5, "a-"), (0x1EA7, "a-"), (0x1EA9, "a-"), (0x1EAB, "a-"), (0x1EAD, "a-"),
    (0x1EAF, "a-"), (0x1EB1, "a-"), (0x1EB3, "a-"), (0x1EB5, "a-"), (0x1EB7, "a-"), (0x1EB9, "e-"), (0x1EBB, "e-"),
    (0x1EBD, "e-"), (0x1EBF, "e-"), (0x1EC1, "e-"), (0x1EC3, "e-"), (0x1EC5, "e-"), (0x1EC7, "e-"), (0x1EC9, "i-"),
    (0x1ECB, "i-"), (0x1ECD, "o-"), (0x1ECF, "o-"), (0x1ED1, "o-"), (0x1ED3, "o-"), (0x1ED5, "o-"), (0x1ED7, "o-"),
    (0x1ED9, "o-"), (0x1EDB, "o-"), (0x1EDD, "o-"), (0x1EDF, "o-"), (0x1EE1, "o-"), (0x1EE3, "o-"), (0x1EE5, "u-"),
    (0x1EE7, "u-"), (0x1EE9, "u-"), (0x1EEB, "u-"), (0x1EED, "u-"), (0x1EEF, "u-"), (0x1EF1, "u-"), (0x1EF3, "y-"),
    (0x1EF5, "y-"), (0x1EF7, "y-"), (0x1EF9, "y-"), (0x2070, "0"), (0x2071, "i"), (0x2074, "4"), (0x2075, "5"),
    (0x2076, "6"), (0x2077, "7"), (0x2078, "8"), (0x2079, "9"), (0x207F, "n"), (0x2080, "0"), (0x2081, "1"),
    (0x2082, "2"), (0x2083, "3"), (0x2084, "4"), (0x2085, "5"), (0x2086, "6"), (0x2087, "7"), (0x2088, "8"),
    (0x2089, "9"), (0x2090, "a"), (0x2091, "e"), (0x2092, "o"), (0x2093, "x"), (0x2095, "h"), (0x2096, "k"),
    (0x2097, "l"), (0x2098, "m"), (0x2099, "n"), (0x209A, "p"), (0x209B, "s"), (0x209C, "t"), (0x20A8, "-s"),
    (0x2100, "a-c"), (0x2101, "a-s"), (0x2105, "c-o"), (0x2106, "c-u"), (0x210A, "g"), (0x210E, "h"), (0x2113, "l"),
    (0x2116, "-o"), (0x212F, "e"), (0x2134, "o"), (0x2139, "i"), (0x2146, "d"), (0x2147, "e"), (0x2148, "i"),
    (0x2149, "j"), (0x2150, "1-7"), (0x2151, "1-9"), (0x2152, "1-10"), (0x2153, "1-3"), (0x2154, "2-3"),
    (0x2155, "1-5"), (0x2156, "2-5"), (0x2157, "3-5"), (0x2158, "4-5"), (0x2159, "1-6"), (0x215A, "5-6"),
    (0x215B, "1-8"), (0x215C, "3-8"), (0x215D, "5-8"), (0x215E, "7-8"), (0x215F, "1-"), (0x2170, "i"), (0x2171, "ii"),
    (0x2172, "iii"), (0x2173, "iv"), (0x2174, "v"), (0x2175, "vi"), (0x2176, "vii"), (0x2177, "viii"), (0x2178, "ix"),
    (0x2179, "x"), (0x217A, "xi"), (0x217B, "xii"), (0x217C, "l"), (0x217D, "c"), (0x217E, "d"), (0x217F, "m"),
    (0x2189, "0-3"), (0x2460, "1"), (0x2461, "2"), (0x2462, "3"), (0x2463, "4"), (0x2464, "5"), (0x2465, "6"),
    (0x2466, "7"), (0x2467, "8"), (0x2468, "9"), (0x2469, "10"), (0x246A, "11"), (0x246B, "12"), (0x246C, "13"),
    (0x246D, "14"), (0x246E, "15"), (0x246F, "16"), (0x2470, "17"), (0x2471, "18"), (0x2472, "19"), (0x2473, "20"),
    (0x2474, "-1-"), (0x2475, "-2-"), (0x2476, "-3-"), (0x2477, "-4-"), (0x2478, "-5-"), (0x2479, "-6-"),
    (0x247A, "-7-"), (0x247B, "-8-"), (0x247C, "-9-"), (0x247D, "-10-"), (0x247E, "-11-"), (0x247F, "-12-"),
    (0x2480, "-13-"), (0x2481, "-14-"), (0x2482, "-15-"), (0x2483, "-16-"), (0x2484, "-17-"), (0x2485, "-18-"),
    (0x2486, "-19-"), (0x2487, "-20-"), (0x2488, "1-"), (0x2489, "2-"), (0x248A, "3-"), (0x248B, "4-"),
    (0x248C, "5-"), (0x248D, "6-"), (0x248E, "7-"), (0x248F, "8-"), (0x2490, "9-"), (0x2491, "10-"), (0x2492, "11-"),
    (0x2493, "12-"), (0x2494, "13-"), (0x2495, "14-"), (0x2496, "15-"), (0x2497, "16-"), (0x2498, "17-"),
    (0x2499, "18-"), (0x249A, "19-"), (0x249B, "20-"), (0x249C, "-a-"), (0x249D, "-b-"), (0x249E, "-c-"),
    (0x249F, "-d-"), (0x24A0, "-e-"), (0x24A1, "-f-"), (0x24A2, "-g-"), (0x24A3, "-h-"), (0x24A4, "-i-"),
    (0x24A5, "-j-"), (0x24A6, "-k-"), (0x24A7, "-l-"), (0x24A8, "-m-"), (0x24A9, "-n-"), (0x24AA, "-o-"),
    (0x24AB, "-p-"), (0x24AC, "-q-"), (0x24AD, "-r-"), (0x24AE, "-s-"), (0x24AF, "-t-"), (0x24B0, "-u-"),
    (0x24B1, "-v-"), (0x24B2, "-w-"), (0x24B3, "-x-"), (0x24B4, "-y-"), (0x24B5, "-z-"), (0x24D0, "a"), (0x24D1, "b"),
    (0x24D2, "c"), (0x24D3, "d"), (0x24D4, "e"), (0x24D5, "f"), (0x24D6, "g"), (0x24D7, "h"), (0x24D8, "i"),
    (0x24D9, "j"), (0x24DA, "k"), (0x24DB, "l"), (0x24DC, "m"), (0x24DD, "n"), (0x24DE, "o"), (0x24DF, "p"),
    (0x24E0, "q"), (0x24E1, "r"), (0x24E2, "s"), (0x24E3, "t"), (0x24E4, "u"), (0x24E5, "v"), (0x24E6, "w"),
    (0x24E7, "x"), (0x24E8, "y"), (0x24E9, "z"), (0x24EA, "0"), (0x2C7C, "j"), (0x3251, "21"), (0x3252, "22"),
    (0x3253, "23"), (0x3254, "24"), (0x3255, "25"), (0x3256, "26"), (0x3257, "27"), (0x3258, "28"), (0x3259, "29"),
    (0x325A, "30"), (0x325B, "31"), (0x325C, "32"), (0x325D, "33"), (0x325E, "34"), (0x325F, "35"), (0x32B1, "36"),
    (0x32B2, "37"), (0x32B3, "38"), (0x32B4, "39"), (0x32B5, "40"), (0x32B6, "41"), (0x32B7, "42"), (0x32B8, "43"),
    (0x32B9, "44"), (0x32BA, "45"), (0x32BB, "46"), (0x32BC, "47"), (0x32BD, "48"), (0x32BE, "49"), (0x32BF, "50"),
    (0x32C0, "1-"), (0x32C1, "2-"), (0x32C2, "3-"), (0x32C3, "4-"), (0x32C4, "5-"), (0x32C5, "6-"), (0x32C6, "7-"),
    (0x32C7, "8-"), (0x32C8, "9-"), (0x32C9, "10-"), (0x32CA, "11-"), (0x32CB, "12-"), (0x32CC, "-g"),
    (0x32CD, "erg"), (0x32CE, "e-"), (0x3358, "0-"), (0x3359, "1-"), (0x335A, "2-"), (0x335B, "3-"), (0x335C, "4-"),
    (0x335D, "5-"), (0x335E, "6-"), (0x335F, "7-"), (0x3360, "8-"), (0x3361, "9-"), (0x3362, "10-"), (0x3363, "11-"),
    (0x3364, "12-"), (0x3365, "13-"), (0x3366, "14-"), (0x3367, "15-"), (0x3368, "16-"), (0x3369, "17-"),
    (0x336A, "18-"), (0x336B, "19-"), (0x336C, "20-"), (0x336D, "21-"), (0x336E, "22-"), (0x336F, "23-"),
    (0x3370, "24-"), (0x3371, "h-a"), (0x3372, "da"), (0x3374, "bar"), (0x3375, "o-"), (0x3376, "pc"), (0x3377, "dm"),
    (0x3378, "dm2"), (0x3379, "dm3"), (0x3380, "p-"), (0x3381, "n-"), (0x3383, "m-"), (0x3384, "k-"), (0x3388, "cal"),
    (0x3389, "kcal"), (0x338A, "p-"), (0x338B, "n-"), (0x338D, "-g"), (0x338E, "mg"), (0x338F, "kg"), (0x3390, "-z"),
    (0x3391, "k-z"), (0x3392, "-z"), (0x3393, "-z"), (0x3394, "-z"), (0x3395, "-l"), (0x3396, "ml"), (0x3397, "dl"),
    (0x3398, "kl"), (0x3399, "fm"), (0x339A, "nm"), (0x339B, "-m"), (0x339C, "mm"), (0x339D, "cm"), (0x339E, "km"),
    (0x339F, "mm2"), (0x33A0, "cm2"), (0x33A1, "m2"), (0x33A2, "km2"), (0x33A3, "mm3"), (0x33A4, "cm3"),
    (0x33A5, "m3"), (0x33A6, "km3"), (0x33A7, "m-s"), (0x33A8, "m-s2"), (0x33A9, "-a"), (0x33AA, "k-a"),
    (0x33AB, "-a"), (0x33AC, "-a"), (0x33AD, "rad"), (0x33AE, "rad-s"), (0x33AF, "rad-s2"), (0x33B0, "ps"),
    (0x33B1, "ns"), (0x33B2, "-s"), (0x33B3, "ms"), (0x33B4, "p-"), (0x33B5, "n-"), (0x33B7, "m-"), (0x33B8, "k-"),
    (0x33BA, "p-"), (0x33BB, "n-"), (0x33BD, "m-"), (0x33BE, "k-"), (0x33C0, "k-"), (0x33C2, "a-m-"), (0x33C3, "-q"),
    (0x33C4, "cc"), (0x33C5, "cd"), (0x33C6, "-kg"), (0x33C7, "-o-"), (0x33C8, "d-"), (0x33C9, "-y"), (0x33CA, "ha"),
    (0x33CC, "in"), (0x33CF, "kt"), (0x33D0, "lm"), (0x33D1, "ln"), (0x33D2, "log"), (0x33D3, "lx"), (0x33D4, "mb"),
    (0x33D5, "mil"), (0x33D6, "mol"), (0x33D8, "p-m-"), (0x33DB, "sr"), (0x33DC, "-v"), (0x33DD, "-b"),
    (0x33DE, "-m"), (0x33DF, "-m"), (0x33E0, "1-"), (0x33E1, "2-"), (0x33E2, "3-"), (0x33E3, "4-"), (0x33E4, "5-"),
    (0x33E5, "6-"), (0x33E6, "7-"), (0x33E7, "8-"), (0x33E8, "9-"), (0x33E9, "10-"), (0x33EA, "11-"), (0x33EB, "12-"),
    (0x33EC, "13-"), (0x33ED, "14-"), (0x33EE, "15-"), (0x33EF, "16-"), (0x33F0, "17-"), (0x33F1, "18-"),
    (0x33F2, "19-"), (0x33F3, "20-"), (0x33F4, "21-"), (0x33F5, "22-"), (0x33F6, "23-"), (0x33F7, "24-"),
    (0x33F8, "25-"), (0x33F9, "26-"), (0x33FA, "27-"), (0x33FB, "28-"), (0x33FC, "29-"), (0x33FD, "30-"),
    (0x33FE, "31-"), (0x33FF, "gal"), (0xFB00, "ff"), (0xFB01, "fi"), (0xFB02, "fl"), (0xFB03, "ffi"),
    (0xFB04, "ffl"), (0xFB05, "st"), (0xFB06, "st"), (0xFF10, "0"), (0xFF11, "1"), (0xFF12, "2"), (0xFF13, "3"),
    (0xFF14, "4"), (0xFF15, "5"), (0xFF16, "6"), (0xFF17, "7"), (0xFF18, "8"), (0xFF19, "9"), (0xFF41, "a"),
    (0xFF42, "b"), (0xFF43, "c"), (0xFF44, "d"), (0xFF45, "e"), (0xFF46, "f"), (0xFF47, "g"), (0xFF48, "h"),
    (0xFF49, "i"), (0xFF4A, "j"), (0xFF4B, "k"), (0xFF4C, "l"), (0xFF4D, "m"), (0xFF4E, "n"), (0xFF4F, "o"),
    (0xFF50, "p"), (0xFF51, "q"), (0xFF52, "r"), (0xFF53, "s"), (0xFF54, "t"), (0xFF55, "u"), (0xFF56, "v"),
    (0xFF57, "w"), (0xFF58, "x"), (0xFF59, "y"), (0xFF5A, "z"), (0x107A5, "q"), (0x1D41A, "a"), (0x1D41B, "b"),
    (0x1D41C, "c"), (0x1D41D, "d"), (0x1D41E, "e"), (0x1D41F, "f"), (0x1D420, "g"), (0x1D421, "h"), (0x1D422, "i"),
    (0x1D423, "j"), (0x1D424, "k"), (0x1D425, "l"), (0x1D426, "m"), (0x1D427, "n"), (0x1D428, "o"), (0x1D429, "p"),
    (0x1D42A, "q"), (0x1D42B, "r"), (0x1D42C, "s"), (0x1D42D, "t"), (0x1D42E, "u"), (0x1D42F, "v"), (0x1D430, "w"),
    (0x1D431, "x"), (0x1D432, "y"), (0x1D433, "z"), (0x1D44E, "a"), (0x1D44F, "b"), (0x1D450, "c"), (0x1D451, "d"),
    (0x1D452, "e"), (0x1D453, "f"), (0x1D454, "g"), (0x1D456, "i"), (0x1D457, "j"), (0x1D458, "k"), (0x1D459, "l"),
    (0x1D45A, "m"), (0x1D45B, "n"), (0x1D45C, "o"), (0x1D45D, "p"), (0x1D45E, "q"), (0x1D45F, "r"), (0x1D460, "s"),
    (0x1D461, "t"), (0x1D462, "u"), (0x1D463, "v"), (0x1D464, "w"), (0x1D465, "x"), (0x1D466, "y"), (0x1D467, "z"),
    (0x1D482, "a"), (0x1D483, "b"), (0x1D484, "c"), (0x1D485, "d"), (0x1D486, "e"), (0x1D487, "f"), (0x1D488, "g"),
    (0x1D489, "h"), (0x1D48A, "i"), (0x1D48B, "j"), (0x1D48C, "k"), (0x1D48D, "l"), (0x1D48E, "m"), (0x1D48F, "n"),
    (0x1D490, "o"), (0x1D491, "p"), (0x1D492, "q"), (0x1D493, "r"), (0x1D494, "s"), (0x1D495, "t"), (0x1D496, "u"),
    (0x1D497, "v"), (0x1D498, "w"), (0x1D499, "x"), (0x1D49A, "y"), (0x1D49B, "z"), (0x1D4B6, "a"), (0x1D4B7, "b"),
    (0x1D4B8, "c"), (0x1D4B9, "d"), (0x1D4BB, "f"), (0x1D4BD, "h"), (0x1D4BE, "i"), (0x1D4BF, "j"), (0x1D4C0, "k"),
    (0x1D4C1, "l"), (0x1D4C2, "m"), (0x1D4C3, "n"), (0x1D4C5, "p"), (0x1D4C6, "q"), (0x1D4C7, "r"), (0x1D4C8, "s"),
    (0x1D4C9, "t"), (0x1D4CA, "u"), (0x1D4CB, "v"), (0x1D4CC, "w"), (0x1D4CD, "x"), (0x1D4CE, "y"), (0x1D4CF, "z"),
    (0x1D4EA, "a"), (0x1D4EB, "b"), (0x1D4EC, "c"), (0x1D4ED, "d"), (0x1D4EE, "e"), (0x1D4EF, "f"), (0x1D4F0, "g"),
    (0x1D4F1, "h"), (0x1D4F2, "i"), (0x1D4F3, "j"), (0x1D4F4, "k"), (0x1D4F5, "l"), (0x1D4F6, "m"), (0x1D4F7, "n"),
    (0x1D4F8, "o"), (0x1D4F9, "p"), (0x1D4FA, "q"), (0x1D4FB, "r"), (0x1D4FC, "s"), (0x1D4FD, "t"), (0x1D4FE, "u"),
    (0x1D4FF, "v"), (0x1D500, "w"), (0x1D501, "x"), (0x1D502, "y"), (0x1D503, "z"), (0x1D51E, "a"), (0x1D51F, "b"),
    (0x1D520, "c"), (0x1D521, "d"), (0x1D522, "e"), (0x1D523, "f"), (0x1D524, "g"), (0x1D525, "h"), (0x1D526, "i"),
    (0x1D527, "j"), (0x1D528, "k"), (0x1D529, "l"), (0x1D52A, "m"), (0x1D52B, "n"), (0x1D52C, "o"), (0x1D52D, "p"),
    (0x1D52E, "q"), (0x1D52F, "r"), (0x1D530, "s"), (0x1D531, "t"), (0x1D532, "u"), (0x1D533, "v"), (0x1D534, "w"),
    (0x1D535, "x"), (0x1D536, "y"), (0x1D537, "z"), (0x1D552, "a"), (0x1D553, "b"), (0x1D554, "c"), (0x1D555, "d"),
    (0x1D556, "e"), (0x1D557, "f"), (0x1D558, "g"), (0x1D559, "h"), (0x1D55A, "i"), (0x1D55B, "j"), (0x1D55C, "k"),
    (0x1D55D, "l"), (0x1D55E, "m"), (0x1D55F, "n"), (0x1D560, "o"), (0x1D561, "p"), (0x1D562, "q"), (0x1D563, "r"),
    (0x1D564, "s"), (0x1D565, "t"), (0x1D566, "u"), (0x1D567, "v"), (0x1D568, "w"), (0x1D569, "x"), (0x1D56A, "y"),
    (0x1D56B, "z"), (0x1D586, "a"), (0x1D587, "b"), (0x1D588, "c"), (0x1D589, "d"), (0x1D58A, "e"), (0x1D58B, "f"),
    (0x1D58C, "g"), (0x1D58D, "h"), (0x1D58E, "i"), (0x1D58F, "j"), (0x1D590, "k"), (0x1D591, "l"), (0x1D592, "m"),
    (0x1D593, "n"), (0x1D594, "o"), (0x1D595, "p"), (0x1D596, "q"), (0x1D597, "r"), (0x1D598, "s"), (0x1D599, "t"),
    (0x1D59A, "u"), (0x1D59B, "v"), (0x1D59C, "w"), (0x1D59D, "x"), (0x1D59E, "y"), (0x1D59F, "z"), (0x1D5BA, "a"),
    (0x1D5BB, "b"), (0x1D5BC, "c"), (0x1D5BD, "d"), (0x1D5BE, "e"), (0x1D5BF, "f"), (0x1D5C0, "g"), (0x1D5C1, "h"),
    (0x1D5C2, "i"), (0x1D5C3, "j"), (0x1D5C4, "k"), (0x1D5C5, "l"), (0x1D5C6, "m"), (0x1D5C7, "n"), (0x1D5C8, "o"),
    (0x1D5C9, "p"), (0x1D5CA, "q"), (0x1D5CB, "r"), (0x1D5CC, "s"), (0x1D5CD, "t"), (0x1D5CE, "u"), (0x1D5CF, "v"),
    (0x1D5D0, "w"), (0x1D5D1, "x"), (0x1D5D2, "y"), (0x1D5D3, "z"), (0x1D5EE, "a"), (0x1D5EF, "b"), (0x1D5F0, "c"),
    (0x1D5F1, "d"), (0x1D5F2, "e"), (0x1D5F3, "f"), (0x1D5F4, "g"), (0x1D5F5, "h"), (0x1D5F6, "i"), (0x1D5F7, "j"),
    (0x1D5F8, "k"), (0x1D5F9, "l"), (0x1D5FA, "m"), (0x1D5FB, "n"), (0x1D5FC, "o"), (0x1D5FD, "p"), (0x1D5FE, "q"),
    (0x1D5FF, "r"), (0x1D600, "s"), (0x1D601, "t"), (0x1D602, "u"), (0x1D603, "v"), (0x1D604, "w"), (0x1D605, "x"),
    (0x1D606, "y"), (0x1D607, "z"), (0x1D622, "a"), (0x1D623, "b"), (0x1D624, "c"), (0x1D625, "d"), (0x1D626, "e"),
    (0x1D627, "f"), (0x1D628, "g"), (0x1D629, "h"), (0x1D62A, "i"), (0x1D62B, "j"), (0x1D62C, "k"), (0x1D62D, "l"),
    (0x1D62E, "m"), (0x1D62F, "n"), (0x1D630, "o"), (0x1D631, "p"), (0x1D632, "q"), (0x1D633, "r"), (0x1D634, "s"),
    (0x1D635, "t"), (0x1D636, "u"), (0x1D637, "v"), (0x1D638, "w"), (0x1D639, "x"), (0x1D63A, "y"), (0x1D63B, "z"),
    (0x1D656, "a"), (0x1D657, "b"), (0x1D658, "c"), (0x1D659, "d"), (0x1D65A, "e"), (0x1D65B, "f"), (0x1D65C, "g"),
    (0x1D65D, "h"), (0x1D65E, "i"), (0x1D65F, "j"), (0x1D660, "k"), (0x1D661, "l"), (0x1D662, "m"), (0x1D663, "n"),
    (0x1D664, "o"), (0x1D665, "p"), (0x1D666, "q"), (0x1D667, "r"), (0x1D668, "s"), (0x1D669, "t"), (0x1D66A, "u"),
    (0x1D66B, "v"), (0x1D66C, "w"), (0x1D66D, "x"), (0x1D66E, "y"), (0x1D66F, "z"), (0x1D68A, "a"), (0x1D68B, "b"),
    (0x1D68C, "c"), (0x1D68D, "d"), (0x1D68E, "e"), (0x1D68F, "f"), (0x1D690, "g"), (0x1D691, "h"), (0x1D692, "i"),
    (0x1D693, "j"), (0x1D694, "k"), (0x1D695, "l"), (0x1D696, "m"), (0x1D697, "n"), (0x1D698, "o"), (0x1D699, "p"),
    (0x1D69A, "q"), (0x1D69B, "r"), (0x1D69C, "s"), (0x1D69D, "t"), (0x1D69E, "u"), (0x1D69F, "v"), (0x1D6A0, "w"),
    (0x1D6A1, "x"), (0x1D6A2, "y"), (0x1D6A3, "z"), (0x1D7CE, "0"), (0x1D7CF, "1"), (0x1D7D0, "2"), (0x1D7D1, "3"),
    (0x1D7D2, "4"), (0x1D7D3, "5"), (0x1D7D4, "6"), (0x1D7D5, "7"), (0x1D7D6, "8"), (0x1D7D7, "9"), (0x1D7D8, "0"),
    (0x1D7D9, "1"), (0x1D7DA, "2"), (0x1D7DB, "3"), (0x1D7DC, "4"), (0x1D7DD, "5"), (0x1D7DE, "6"), (0x1D7DF, "7"),
    (0x1D7E0, "8"), (0x1D7E1, "9"), (0x1D7E2, "0"), (0x1D7E3, "1"), (0x1D7E4, "2"), (0x1D7E5, "3"), (0x1D7E6, "4"),
    (0x1D7E7, "5"), (0x1D7E8, "6"), (0x1D7E9, "7"), (0x1D7EA, "8"), (0x1D7EB, "9"), (0x1D7EC, "0"), (0x1D7ED, "1"),
    (0x1D7EE, "2"), (0x1D7EF, "3"), (0x1D7F0, "4"), (0x1D7F1, "5"), (0x1D7F2, "6"), (0x1D7F3, "7"), (0x1D7F4, "8"),
    (0x1D7F5, "9"), (0x1D7F6, "0"), (0x1D7F7, "1"), (0x1D7F8, "2"), (0x1D7F9, "3"), (0x1D7FA, "4"), (0x1D7FB, "5"),
    (0x1D7FC, "6"), (0x1D7FD, "7"), (0x1D7FE, "8"), (0x1D7FF, "9"), (0x1F100, "0-"), (0x1F101, "0-"), (0x1F102, "1-"),
    (0x1F103, "2-"), (0x1F104, "3-"), (0x1F105, "4-"), (0x1F106, "5-"), (0x1F107, "6-"), (0x1F108, "7-"),
    (0x1F109, "8-"), (0x1F10A, "9-"), (0x1FBF0, "0"), (0x1FBF1, "1"), (0x1FBF2, "2"), (0x1FBF3, "3"), (0x1FBF4, "4"),
    (0x1FBF5, "5"), (0x1FBF6, "6"), (0x1FBF7, "7"), (0x1FBF8, "8"), (0x1FBF9, "9"),
];
fn nfkd_ascii(c: char) -> Option<&'static str> {
    NFKD.binary_search_by_key(&(c as u32), |e| e.0).ok().map(|i| NFKD[i].1)
}

// "Billing API!" -> "billing-api". Used once, when a project is created; the slug names its artifact folder.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    let mut push = |c: char| {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    };
    for c in name.to_lowercase().chars() {
        match (c.is_ascii(), nfkd_ascii(c)) {
            (false, Some(s)) => s.chars().for_each(&mut push),
            _ => push(c),
        }
    }
    if out.is_empty() {
        "project".into()
    } else {
        out
    }
}

// The slug itself if free, else slug-2, slug-3, ...
pub fn unique_slug(slug: &str, taken: &HashSet<String>) -> String {
    let mut s = slug.to_string();
    let mut n = 2;
    while taken.contains(&s) {
        s = format!("{slug}-{n}");
        n += 1;
    }
    s
}

// A turn's final text shown as a chat message: at most two lines and 280 characters,
// because one long paragraph is a single line too.
pub fn cut_to_two_lines(text: &str) -> String {
    let lines: Vec<&str> = js_trim(text).split('\n').map(js_trim).filter(|l| !l.is_empty()).collect();
    let mut out = lines[..lines.len().min(2)].join("\n");
    if js_len(&out) > 280 {
        out = js_head(&out, 279).trim_end_matches(js_ws).to_string();
    }
    if js_len(&out) < js_len(&lines.join("\n")) {
        format!("{out}…")
    } else {
        out
    }
}

// Why a failed turn is worth continuing: the Mac slept mid-request, or the API failed for a while. None for every other
// error (a session limit, a login, a dead agent), which a retry would not fix.
pub fn cut_reason(error: &str) -> Option<String> {
    static API_5XX: LazyLock<Regex> = LazyLock::new(|| re(r"API Error: 5[0-9][0-9](?-u:\b)"));
    if error.contains("went to sleep mid-response") {
        return Some("the computer slept".into());
    }
    if error.contains("Request timed out") {
        return Some("the request timed out".into());
    }
    if API_5XX.is_match(error) {
        return Some("an API server error".into());
    }
    None
}

pub fn line_count(text: &str) -> i64 {
    js_trim(text).split('\n').filter(|l| !js_trim(l).is_empty()).count() as i64
}

// The agent's suggested follow-ups (pills): one or two one-line instructions the user sends with a click.
pub fn follow_up_error(items: &[String]) -> Option<String> {
    if items.is_empty() || items.len() > 2 {
        return Some("Suggest one or two follow-ups.".into());
    }
    for s in items {
        let t = js_trim(s);
        if t.is_empty() {
            return Some("A follow-up is empty.".into());
        }
        if t.contains('\n') {
            return Some("A follow-up is one line.".into());
        }
        if js_len(t) > 80 {
            return Some("A follow-up is at most 80 characters.".into());
        }
    }
    None
}

// The prompt for one turn: the worktree line (while one is registered), then every delivered message.
// With no messages, as on a restore with nothing queued and no draft, the prompt is "continue".
// The first turn of a later session starts with the hand-over header instead, which names the worktree itself.
// `header` is "" for none.
pub fn delivery_prompt(worktree: Option<&str>, bodies: &[String], header: &str) -> String {
    let text = bodies.iter().map(|b| js_trim(b)).filter(|b| !b.is_empty()).collect::<Vec<_>>().join("\n\n");
    let text = if text.is_empty() { "continue".to_string() } else { text };
    if !header.is_empty() {
        return format!("{header}\n\n{text}");
    }
    match worktree.filter(|w| !w.is_empty()) {
        Some(w) => format!("Your worktree: {w}. Work there.\n\n{text}"),
        None => text,
    }
}

// Change topic: a TODO's worktree, branch and PR per topic it left, keyed by project id ("none": no topic).
// A Place is { worktree_path, branch, pr_number }.
// Stores the current place under the topic left and takes the one of the topic entered, if it had one. A stored worktree
// whose folder is gone (`exists` false) comes back without it. Returns { places, back: { worktree_path, pr_number } }.
pub fn swap_place(places: &Value, from: Option<i64>, to: Option<i64>, cur: &Value, exists: impl Fn(&str) -> bool) -> Value {
    let key = |id: Option<i64>| id.map_or("none".to_string(), |i| i.to_string());
    let mut next = js_spread(places);
    if cur.truthy("worktree_path") || cur.truthy("pr_number") {
        next.insert(key(from), cur.clone());
    } else {
        next.shift_remove(&key(from));
    }
    let b = next.shift_remove(&key(to));
    let wt = match b.as_ref().and_then(|b| b.get("worktree_path")) {
        Some(w) if truthy(w) && exists(&js_string(Some(w))) => w.clone(),
        _ => Value::Null,
    };
    let pr = b.as_ref().and_then(|b| nn(b.get("pr_number"))).cloned().unwrap_or(Value::Null);
    json!({ "places": Value::Object(js_order(next)), "back": { "worktree_path": wt, "pr_number": pr } })
}

/// `s.replace(/\s*\n\s*/g, " ")`: each whitespace run holding a line break becomes one space.
fn one_line(s: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |out: &mut String, run: &mut String| {
        if run.contains('\n') {
            out.push(' ');
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in s.chars() {
        if js_ws(c) {
            run.push(c);
        } else {
            flush(&mut out, &mut run);
            out.push(c);
        }
    }
    flush(&mut out, &mut run);
    out
}

// Item 73: what a new session gets from the earlier ones, built from the TODO's data (no agent writes it).
// Lines without a value are left out. `scheduled` is the run line of a scheduled firing (item 144).
// `o`: { no, title, worktree, pr, last: { no, body } | null, files: [{ path, session_no, by_user? }], work?, scheduled? }
pub fn hand_over(o: &Value) -> String {
    let s = |k: &str| js_string(o.get(k));
    let mut lines = vec![format!("This TODO continues earlier work (session {}). Title: {}. Keep this title.", s("no"), s("title"))];
    if o.truthy("scheduled") {
        lines.push(format!("Scheduled run: {}.", s("scheduled")));
    }
    if o.truthy("worktree") {
        lines.push(format!("Your worktree: {}. Work there.", s("worktree")));
    }
    if o.truthy("pr") {
        lines.push(format!("Pull request: #{}.", s("pr")));
    }
    if let Some(last) = o.get("last").filter(|l| truthy(l)) {
        lines.push(format!("Last message of session {}: {}", js_string(last.get("no")), one_line(&js_string(last.get("body")))));
    }
    let files = arr(o.get("files"));
    if !files.is_empty() {
        let list = files.iter().map(|f| {
            let by = if f.truthy("by_user") {
                "the user's notes".to_string()
            } else if f.truthy("session_no") {
                format!("session {}", js_string(f.get("session_no")))
            } else {
                "added outside a session".to_string()
            };
            format!("{} ({by})", js_string(f.get("path")))
        });
        lines.push(format!("Files from earlier sessions (open with Read what you need):\n{}", list.collect::<Vec<_>>().join("\n")));
    }
    if o.truthy("work") {
        lines.push(format!("Earlier output for the agent itself (logs, raw results, scripts) is in {}.", s("work")));
    }
    lines.join("\n")
}

// The context meter (item 123), from the agent's ACP usage_update: `used` is the context of the last model call, `size`
// the model's window. Claude names the model in `_meta["_claude/model"]`. last_in and last_out are no longer reported.
// A Ctx is { used, window, last_in?, last_out?, turns, model? }; `c` is Null before the first one.
pub fn ctx_step(c: &Value, u: &Value) -> Value {
    if u.s("sessionUpdate") != Some("usage_update") {
        return c.clone();
    }
    // Only the main loop's own context: Claude marks other cycles (a background task's follow-up, a subagent) by origin.
    if let Some(origin) = at(u, &["_meta", "_claude/origin", "kind"]) {
        if truthy(origin) && origin != "human" && origin != "channel" {
            return c.clone();
        }
    }
    let mut cur = if c.is_null() { js_spread(&json!({ "used": 0, "window": null, "turns": 0 })) } else { js_spread(c) };
    let used = nn(u.get("used")).or(cur.get("used")).cloned();
    put(&mut cur, "used", used.as_ref());
    let window = nn(u.get("size")).or(cur.get("window")).cloned();
    put(&mut cur, "window", window.as_ref());
    let model = nn(at(u, &["_meta", "_claude/model"])).or(cur.get("model")).cloned();
    put(&mut cur, "model", model.as_ref());
    Value::Object(cur)
}

// ---------- background tasks (item 136) ----------

// The agent's background tasks still running, oldest first, kept from its ACP updates (spike 2026-10-09). A Bash call
// with run_in_background ends at once with `backgroundTaskId` in its result; TaskStop (KillShell before it) names the
// task it stopped. How a task ends otherwise (it exits, a plain `kill`) only the SDK's level says, below.
// PORT: JS returns the same array when nothing changed; callers here compare by value.
pub fn background_step(ids: &[String], u: &Value) -> Vec<String> {
    let cc = at(u, &["_meta", "claudeCode"]);
    let res = cc.and_then(|c| c.get("toolResponse")).filter(|r| truthy(r));
    if let (Some("tool_call_update"), Some(res)) = (u.s("sessionUpdate"), res) {
        if let Some(id) = res.s("backgroundTaskId") {
            return if ids.iter().any(|i| i == id) { ids.to_vec() } else { [ids, &[id.to_string()]].concat() };
        }
        let stopped = nn(res.get("task_id")).or(res.get("shell_id")).and_then(Value::as_str);
        let tool = cc.and_then(|c| c.s("toolName"));
        if let Some(stopped) = stopped {
            if matches!(tool, Some("TaskStop" | "KillShell")) && ids.iter().any(|i| i == stopped) {
                return ids.iter().filter(|i| *i != stopped).cloned().collect();
            }
        }
    }
    ids.to_vec()
}

fn level_tasks(m: &Value) -> Option<&Vec<Value>> {
    if m.s("type") != Some("system") || m.s("subtype") != Some("background_tasks_changed") {
        return None;
    }
    m.get("tasks").and_then(Value::as_array)
}
fn shell_task(t: &Value) -> bool {
    t.s("task_type") == Some("local_bash") && !t.truthy("ambient")
}

// The SDK's background_tasks_changed, forwarded raw by the adapter when the session asks for it (spike 2026-10-11):
// the full set of live background tasks on every change, so it replaces the list. A task killed by a plain `kill`
// leaves it at once, with no follow-up cycle. Only shell tasks count, as above; `ambient` ones are not activity.
// PORT: JS returns the same array when nothing changed; callers here compare by value.
pub fn background_level(ids: &[String], m: &Value) -> Vec<String> {
    let Some(tasks) = level_tasks(m) else { return ids.to_vec() };
    let live: Vec<String> = tasks.iter().filter(|t| shell_task(t)).map(|t| js_string(t.get("task_id"))).collect();
    let mut next: Vec<String> = ids.iter().filter(|id| live.contains(id)).cloned().collect();
    next.extend(live.iter().filter(|id| !ids.contains(id)).cloned());
    next
}

// The names the SDK's level gives the tasks (their Bash `description`), kept by id; only the shell tasks of
// background_level, so a subagent starting does not refetch the board. A task that joined by its Bash result before the
// level arrived has no name until then. `names` is {task id: description}.
pub fn background_names(names: &Value, m: &Value) -> Value {
    let Some(tasks) = level_tasks(m) else { return names.clone() };
    let add: Vec<&Value> = tasks
        .iter()
        .filter(|t| {
            shell_task(t)
                && t.truthy("description")
                && !names.get(js_string(t.get("task_id"))).is_some_and(|n| t.get("description").is_some_and(|d| same(n, d)))
        })
        .collect();
    if add.is_empty() {
        return names.clone();
    }
    let mut out = js_spread(names);
    for t in add {
        out.insert(js_string(t.get("task_id")), json!(js_string(t.get("description"))));
    }
    Value::Object(js_order(out))
}

// ---------- ports (the Running view's Scan) ----------

/// The first character of a line (as an ASCII byte) and the rest, as `line[0]` and `line.slice(1)` read it.
fn first_and_rest(line: &str) -> Option<(u8, &str)> {
    line.as_bytes().first().filter(|b| b.is_ascii()).map(|b| (*b, &line[1..]))
}

// `lsof -nP -iTCP -sTCP:LISTEN -Fpcn`: a `p<pid>` line, a `c<name>` line, then an `f`/`n<addr:port>` pair per socket.
// One entry per process, its ports ascending (IPv4 and IPv6 of one port count once). Returns [{ pid, name, ports }].
pub fn parse_listen(out: &str) -> Value {
    let mut procs: Vec<(f64, String, Vec<f64>)> = vec![];
    let mut cur: Option<usize> = None;
    for line in out.split('\n') {
        let Some((first, v)) = first_and_rest(line) else { continue };
        match first {
            b'p' => {
                let pid = js_number(v);
                cur = Some(match procs.iter().position(|p| p.0 == pid || (p.0.is_nan() && pid.is_nan())) {
                    Some(i) => i,
                    None => {
                        procs.push((pid, String::new(), vec![]));
                        procs.len() - 1
                    }
                });
            }
            b'c' => {
                if let Some(i) = cur {
                    procs[i].1 = v.to_string();
                }
            }
            b'n' => {
                if let Some(i) = cur {
                    let port = js_number(&v[v.rfind(':').map_or(0, |i| i + 1)..]);
                    if num_truthy(port) && !procs[i].2.contains(&port) {
                        procs[i].2.push(port);
                    }
                }
            }
            _ => {}
        }
    }
    Value::Array(
        procs
            .into_iter()
            .filter(|p| !p.2.is_empty())
            .map(|(pid, name, mut ports)| {
                ports.sort_by(|a, b| a.total_cmp(b));
                json!({ "pid": num(pid), "name": name, "ports": ports.into_iter().map(num).collect::<Vec<_>>() })
            })
            .collect(),
    )
}

// `lsof -a -p <pids> -d cwd -Fn`: `p<pid>`, then `fcwd`, then `n<folder>`.
pub fn parse_cwd(out: &str) -> HashMap<i64, String> {
    let mut dirs = HashMap::new();
    let mut pid = 0.0;
    for line in out.split('\n') {
        match first_and_rest(line) {
            Some((b'p', v)) => pid = js_number(v),
            Some((b'n', v)) if num_truthy(pid) => {
                dirs.insert(pid as i64, v.to_string());
            }
            _ => {}
        }
    }
    dirs
}

// `ps -A -o pid=,etime=,command=`: seconds since start (etime is "[[dd-]hh:]mm:ss") and the full command per pid,
// as { secs, command }.
pub fn parse_ps(out: &str) -> HashMap<i64, Value> {
    static ROW: LazyLock<Regex> = LazyLock::new(|| {
        re(&format!(r"^([0-9]+)[{WS}]+(?:([0-9]+)-)?(?:([0-9]+):)?([0-9]+):([0-9]+)[{WS}]+([^\n\r\x{{2028}}\x{{2029}}]*)$"))
    });
    let mut rows = HashMap::new();
    for line in out.split('\n') {
        if let Some(m) = ROW.captures(js_trim(line)) {
            let n = |i: usize| m.get(i).map_or(0.0, |x| js_number(x.as_str()));
            let secs = ((n(2) * 24.0 + n(3)) * 60.0 + n(4)) * 60.0 + n(5);
            rows.insert(n(1) as i64, json!({ "secs": num(secs), "command": &m[6] }));
        }
    }
    rows
}

// `docker ps --format` with DOCKER_FORMAT: one container per line. Only published ports count: "0.0.0.0:8000->8000/tcp,
// [::]:8000->8000/tcp" is 8000; an unpublished "8443/tcp" is not on the host.
pub const DOCKER_FORMAT: &str = r#"{{.Names}}|{{.Ports}}|{{.Label "com.docker.compose.project.working_dir"}}|{{.RunningFor}}|{{.Image}}"#;
// Returns [{ name, ports, folder, age, image }].
pub fn parse_docker(out: &str) -> Value {
    static HOST_PORT: LazyLock<Regex> = LazyLock::new(|| re(r":([0-9]+)->"));
    Value::Array(
        out.split('\n')
            .filter(|l| l.contains('|'))
            .map(|l| {
                let f: Vec<&str> = l.split('|').collect();
                let get = |i: usize| f.get(i).copied().unwrap_or("");
                let mut ports: Vec<f64> = vec![];
                for m in HOST_PORT.captures_iter(get(1)) {
                    let p = js_number(&m[1]);
                    if !ports.contains(&p) {
                        ports.push(p);
                    }
                }
                ports.sort_by(|a, b| a.total_cmp(b));
                json!({ "name": get(0), "ports": ports.into_iter().map(num).collect::<Vec<_>>(), "folder": get(2), "age": get(3), "image": get(4) })
            })
            .collect(),
    )
}

// Whose folder a process or container runs in: the TODO whose worktree holds it, else the topic whose folder does.
// The deepest folder wins, so a worktree inside the topic folder beats the topic.
// ponytail: by folder only. A process started from `/` or that changed folder matches nothing; the process tree
// below an agent process would catch it while that agent lives.
// `todos`: [{ id, worktree_path }], `projects`: [{ id, local_path }]. Returns { todo_id, project_id }.
pub fn owner_of(folder: &str, todos: &Value, projects: &Value) -> Value {
    if folder.is_empty() {
        return json!({ "todo_id": null, "project_id": null });
    }
    let deepest = |rows: &Value, k: &str| -> Value {
        let mut best: Option<(&Value, usize)> = None;
        for r in arr(Some(rows)) {
            let Some(dir) = r.s(k).filter(|d| !d.is_empty()) else { continue };
            if (folder == dir || is_inside(folder, dir)) && best.is_none_or(|(_, l)| js_len(dir) > l) {
                best = Some((r, js_len(dir)));
            }
        }
        best.and_then(|(r, _)| nn(r.get("id"))).cloned().unwrap_or(Value::Null)
    };
    json!({ "todo_id": deepest(todos, "worktree_path"), "project_id": deepest(projects, "local_path") })
}

// Between turns the agent can work on its own, e.g. a background task's follow-up. Such a cycle starts with any of
// these updates (not a title or a command list) and ends with a usage_update whose origin is not the user's turn.
pub fn agent_activity(u: &Value) -> bool {
    matches!(u.s("sessionUpdate"), Some("agent_message_chunk" | "agent_thought_chunk" | "tool_call" | "tool_call_update" | "plan"))
        || cycle_end(u)
        || (u.s("sessionUpdate") == Some("usage_update") && !at(u, &["_meta", "_claude/origin"]).is_some_and(truthy))
}
pub fn cycle_end(u: &Value) -> bool {
    if u.s("sessionUpdate") != Some("usage_update") {
        return false;
    }
    at(u, &["_meta", "_claude/origin", "kind"]).is_some_and(|k| truthy(k) && k != "human" && k != "channel")
}

// ---------- transcript (item 130) ----------

fn cut(text: &str, max: usize) -> String {
    let n = js_len(text);
    if n > max {
        format!("{}\n… {} more characters", js_head(text, max), n - max)
    } else {
        text.to_string()
    }
}
fn cut_value(v: &Value, max: usize) -> Value {
    match v {
        Value::String(s) => json!(cut(s, max)),
        other => other.clone(),
    }
}

// One ACP tool call as a transcript row's meta, merged from its tool_call and tool_call_update notifications. Text
// output is cut to 8 KB; diffs are kept up to 200 KB per side, enough for the code an agent writes in one call.
// A ToolMeta is { toolCallId, name, title, kind, status, input, locations: [{ path, line? }],
// diffs: [{ path, oldText, newText }], output }; `prev` is None for the first notification.
pub fn tool_meta(prev: Option<&Value>, u: &Value) -> Value {
    let mut m = if let Some(prev) = prev.filter(|p| truthy(p)) {
        js_spread(prev)
    } else {
        let name = nn(at(u, &["_meta", "claudeCode", "toolName"])).or(nn(u.get("name"))).or(nn(u.get("title"))).cloned().unwrap_or(json!("tool"));
        let mut m = Map::new();
        put(&mut m, "toolCallId", u.get("toolCallId"));
        m.insert("name".into(), name);
        for (k, v) in [("title", json!("")), ("kind", json!("other")), ("status", json!("pending")), ("input", json!({})),
            ("locations", json!([])), ("diffs", json!([])), ("output", json!(""))] {
            m.insert(k.into(), v);
        }
        m
    };
    for k in ["title", "kind", "status"] {
        if u.truthy(k) {
            m.insert(k.into(), u[k].clone());
        }
    }
    if let Some(raw) = u.get("rawInput").filter(|r| matches!(r, Value::Object(o) if !o.is_empty()) || matches!(r, Value::Array(a) if !a.is_empty())) {
        let mut input = js_spread(m.get("input").unwrap_or(&Value::Null));
        input.extend(js_spread(raw));
        m.insert("input".into(), Value::Object(js_order(input)));
    }
    if let Some(Value::Array(locs)) = u.get("locations") {
        if !locs.is_empty() {
            let locs = locs.iter().map(|l| {
                let mut o = Map::new();
                put(&mut o, "path", l.get("path"));
                if l.truthy("line") {
                    o.insert("line".into(), l["line"].clone());
                }
                Value::Object(o)
            });
            m.insert("locations".into(), Value::Array(locs.collect()));
        }
    }
    let done = matches!(u.s("status"), Some("completed" | "failed"));
    if let Some(Value::Array(content)) = u.get("content") {
        let diffs: Vec<Value> = content
            .iter()
            .filter(|c| c.s("type") == Some("diff"))
            .map(|d| {
                let mut o = Map::new();
                put(&mut o, "path", d.get("path"));
                o.insert("oldText".into(), nn(d.get("oldText")).map_or(Value::Null, |t| cut_value(t, 200_000)));
                o.insert("newText".into(), nn(d.get("newText")).map_or(json!(""), |t| cut_value(t, 200_000)));
                Value::Object(o)
            })
            .collect();
        if !diffs.is_empty() {
            m.insert("diffs".into(), Value::Array(diffs));
        }
        let text = content
            .iter()
            .filter(|c| c.s("type") == Some("content") && at(c, &["content", "type"]).is_some_and(|t| t == "text"))
            .map(|c| at(c, &["content", "text"]).map_or(String::new(), join_elem))
            .collect::<Vec<_>>()
            .join("\n");
        // While the call runs the content is its description; its result replaces it.
        if !text.is_empty() && (done || !m.get("output").is_some_and(truthy)) {
            m.insert("output".into(), json!(cut(&text, 8000)));
        }
    }
    if let Some(raw) = u.s("rawOutput").filter(|r| !r.is_empty() && done) {
        m.insert("output".into(), json!(cut(raw, 8000)));
    }
    Value::Object(m)
}

// The agent's plan (its todo list, entries { content, status, priority? }) as `done/total`, or None without one.
pub fn plan_count(entries: &Value) -> Option<String> {
    let e = entries.as_array().filter(|e| !e.is_empty())?;
    Some(format!("{}/{}", e.iter().filter(|x| x.s("status") == Some("completed")).count(), e.len()))
}

// One message as the agent gets it: the text, then the absolute paths of the files the user attached to it.
pub fn with_attachments(body: &str, paths: &[String]) -> String {
    if paths.is_empty() {
        return body.to_string();
    }
    let files = format!("Attached files (open with Read):\n{}", paths.join("\n"));
    match js_trim(body) {
        "" => files,
        b => format!("{b}\n\n{files}"),
    }
}

// "../My Shot (1).png" -> "My-Shot-1-.png": an uploaded file's name as one safe path segment, at most 120 characters
// (the end is kept, so the extension stays).
pub fn safe_file_name(name: &str) -> String {
    let mut s = String::new();
    let mut run = false;
    for c in js_basename(name).chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            s.push(c);
            run = false;
        } else if !run {
            s.push('-');
            run = true;
        }
    }
    let s = js_tail(&s, 120).trim_start_matches(['.', '-']);
    if s.is_empty() {
        "file".into()
    } else {
        s.into()
    }
}

// What a committed message does, by the TODO's status (item 20) and whether a turn runs (`busy`):
// not started -> stored until start; a turn running -> queued, and handed to the turn at its next tool boundary or sent
// when it ends; otherwise sent now. A TODO that shows
// running only because its agent waits for background tasks (item 136) is not busy: the agent can answer now.
pub fn delivery_action(status: &str, busy: bool) -> &'static str {
    if status == "not_started" {
        "pending"
    } else if busy {
        "queue"
    } else {
        "send"
    }
}

// Item 20: a message committed during a turn goes to it at a tool boundary, the way typing into Claude Code does: when a
// tool call ends and no other call of the same batch (the calls the agent made together) still runs. Steering at any
// other moment cuts off a tool call being written, or moves a running command to the background and can end the turn
// early (designs/acp/acp-spike.md).
pub fn at_tool_boundary(batch: &[Option<String>]) -> bool {
    !batch.is_empty() && batch.iter().all(|s| matches!(s.as_deref(), Some("completed" | "failed")))
}

// A run that ended (normally, with an error, or by interrupt) leaves the TODO waiting while the flag is raised.
pub fn status_after_run(attention: bool) -> &'static str {
    if attention {
        "waiting"
    } else {
        "idle"
    }
}

// ---------- permissions (P3) ----------

// The permission modes a project or TODO can run in: Claude's `mode` config option (spike finding 9), `auto` first
// because it is the default. The agent itself may not offer all of them (runner.rs reports when one cannot be set).
pub const MODES: &[&str] = & ["auto", "default", "acceptEdits", "plan", "bypassPermissions"];

// An open session/request_permission, as stored in todos.permission and shown on the card and in the chat: the tool
// call's name, title, kind, input and diffs (as in a transcript row), plus the agent's options. One per TODO at most,
// because the agent blocks until it is answered.
// A Permission is { id, name, title, kind, input, diffs, at, options: [{ optionId, name, kind }] }; `r` is the
// request, { toolCall, options }.
pub fn permission_of(r: &Value, at: &str) -> Value {
    let m = tool_meta(None, nn(r.get("toolCall")).unwrap_or(&json!({})));
    let mut p = Map::new();
    for (k, from) in [("id", "toolCallId"), ("name", "name"), ("title", "title"), ("kind", "kind"), ("input", "input"), ("diffs", "diffs")] {
        put(&mut p, k, m.get(from));
    }
    p.insert("at".into(), json!(at));
    let options = arr(nn(r.get("options"))).iter().map(|o| {
        json!({
            "optionId": js_string(o.get("optionId")),
            "name": js_string(nn(o.get("name")).or(o.get("optionId"))),
            "kind": nn(o.get("kind")).map_or(String::new(), |k| js_string(Some(k))),
        })
    });
    p.insert("options".into(), Value::Array(options.collect()));
    Value::Object(p)
}

// The board lane: a running TODO with an open permission request needs the user now, so it shows in Waiting. So does
// one that runs only for background tasks (item 136) after the agent raised attention itself: the agent is done and
// asks, a Monitor of its own still runs. Otherwise the lane is the status. The tab title and ⌘J count lanes, not statuses.
// A scheduled TODO that is idle or not started is in the Scheduled lane (item 144); running or waiting, it is where any TODO is.
// `t`: { status, permission, background_wait?, attention_by?, schedule? }
pub fn lane_of(t: &Value) -> String {
    let status = t.s("status");
    if status == Some("running") && (t.truthy("permission") || (t.truthy("background_wait") && t.s("attention_by") == Some("agent"))) {
        return "waiting".into();
    }
    if t.truthy("schedule") && matches!(status, Some("idle" | "not_started")) {
        return "scheduled".into();
    }
    js_string(t.get("status"))
}

// Answering a request clears the attention flag only when the request raised it (`attention_by`); a flag the agent
// or a denial raised stays until they or the user clear it.
pub fn attention_after_answer(by: Option<&str>) -> i64 {
    if by == Some("permission") {
        0
    } else {
        1
    }
}

// ---------- config options (P4) ----------

// A session config option as the UI shows it: the agent's option with its choices flattened (ACP allows groups).
// A ConfigOption is { id, name, category, value, options: [{ value, name }] }.
pub fn config_view(raw: &Value) -> Value {
    let view = arr(Some(raw)).iter().filter(|o| o.s("type") == Some("select") && o.truthy("id")).map(|o| {
        let mut flat: Vec<&Value> = vec![];
        for x in arr(nn(o.get("options"))) {
            match nn(x.get("options")) {
                Some(Value::Array(a)) => flat.extend(a),
                Some(v) => flat.push(v),
                None => flat.push(x),
            }
        }
        json!({
            "id": js_string(o.get("id")),
            "name": js_string(nn(o.get("name")).or(o.get("id"))),
            "category": nn(o.get("category")).cloned().unwrap_or(Value::Null),
            "value": nn(o.get("currentValue")).map_or(String::new(), |v| js_string(Some(v))),
            "options": flat.iter().map(|x| json!({
                "value": js_string(x.get("value")),
                "name": js_string(nn(x.get("name")).or(x.get("value"))),
            })).collect::<Vec<_>>(),
        })
    });
    Value::Array(view.collect())
}

// The options a user sets per project and per TODO: model, effort (thought_level) and the like (fast, model_config).
// The permission mode has its own setting (P3); options without a category (Claude's `agent`) are left to the agent.
pub const CONFIG_CATEGORIES: [&str; 3] = ["model", "thought_level", "model_config"];

fn offers(o: &Value, value: &Value) -> bool {
    arr(o.get("options")).iter().any(|x| x.get("value").is_some_and(|v| same(v, value)))
}

// What to set for the wanted values ({config id: value}, the project's defaults under the TODO's own): the changes
// against the agent's current options, and the values it does not offer (effort on a model without it). `models`
// picks the model-category options only, or every other one: a model switch rewrites the others (P4 probe), so
// runner.rs sets the model first and the rest against the options that switch left.
// Returns { set: [[id, value]], skipped: [[id, value]] }.
pub fn config_changes(current: &Value, wanted: &Value, models: bool) -> Value {
    let (mut set, mut skipped) = (vec![], vec![]);
    for (id, value) in js_entries(wanted) {
        let o = arr(Some(current)).iter().find(|c| c.get("id").is_some_and(|x| *x == id));
        if id == "mode" || o.is_some_and(|o| o.s("category") == Some("model")) != models {
            continue; // an option the agent lacks: the second pass
        }
        match o {
            Some(o) if offers(o, &value) => {
                if !o.get("value").is_some_and(|v| same(v, &value)) {
                    set.push(json!([id, value]));
                }
            }
            _ => skipped.push(json!([id, value])),
        }
    }
    json!({ "set": set, "skipped": skipped })
}

// Auto effort (Claude only): the levels the router and the worker may pick, low to high. max stays a user choice.
pub const AUTO_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];
// The effort option's id and the value auto sets: the picked level (None: medium), else the next lower one the agent
// offers, as [id, value]. Null when the wanted values set effort already (the TODO's or its topic's) or the agent has no
// effort option.
pub fn auto_effort(current: &Value, wanted: &Value, level: Option<&str>) -> Value {
    let Some(o) = arr(Some(current)).iter().find(|c| c.s("category") == Some("thought_level")) else { return Value::Null };
    if wanted.get(js_string(o.get("id"))).is_some_and(truthy) {
        return Value::Null;
    }
    let top = level.and_then(|l| AUTO_EFFORTS.iter().position(|x| *x == l)).unwrap_or(1);
    match AUTO_EFFORTS[..=top].iter().rev().find(|x| offers(o, &json!(x))) {
        Some(v) => json!([o.get("id"), v]),
        None => Value::Null,
    }
}

// A {config id: value} object from a request body or a column: string values only, empty ones dropped.
pub fn config_in(v: &Value) -> Value {
    let Value::Object(m) = v else { return json!({}) };
    let kept = m.iter().filter(|(k, x)| *k != "mode" && x.as_str().is_some_and(|s| !s.is_empty()));
    Value::Object(js_order(kept.map(|(k, x)| (k.clone(), x.clone())).collect()))
}

// ---------- agents (P5) ----------

// A project's defaults per agent, {agent id: {config id: value, mode: permission mode}}, as stored in projects.defaults.
// Config ids and modes are the agent's own, so each agent keeps its own set.
// PORT: text that is not JSON reads as no defaults; JSON.parse threw there (the column only ever holds JSON).
pub fn defaults_of(raw: Option<&str>) -> Value {
    let d = raw.filter(|r| !r.is_empty()).and_then(|r| serde_json::from_str::<Value>(r).ok());
    let Some(Value::Object(d)) = d else { return json!({}) };
    let agents = d.into_iter().filter_map(|(agent, v)| {
        let Value::Object(v) = v else { return None };
        let kept = v.into_iter().filter(|(_, x)| x.as_str().is_some_and(|s| !s.is_empty()));
        Some((agent, Value::Object(js_order(kept.collect()))))
    });
    Value::Object(js_order(agents.collect()))
}
// One agent's defaults: its config values (without the mode) and its permission mode, as { config, mode }.
pub fn defaults_for(raw: Option<&str>, agent: &str) -> Value {
    let d = nn(defaults_of(raw).get(agent)).cloned().unwrap_or(json!({}));
    json!({ "config": config_in(&d), "mode": d.s("mode").filter(|m| !m.is_empty()) })
}

// True when `path` is inside `dir` (not `dir` itself). Both must already be absolute and resolved.
pub fn is_inside(path: &str, dir: &str) -> bool {
    // node: rel = relative(dir, path); rel !== "" && !rel.startsWith("..") && !isAbsolute(rel)
    let (p, d) = (resolved(path), resolved(dir));
    p.len() > d.len() && p[..d.len()] == d[..] && !p[d.len()].starts_with("..")
}

// The launch token a request carries (item 120): the X-Switchboard-Token header, or ?token= where a header cannot be
// set (EventSource, WebSocket, <img>). Compared in constant time.
pub fn token_ok(token: &str, header: Option<&str>, query: Option<&str>) -> bool {
    let got: Vec<u16> = header.or(query).unwrap_or("").encode_utf16().collect();
    let want: Vec<u16> = token.encode_utf16().collect();
    if got.len() != want.len() {
        return false;
    }
    got.iter().zip(&want).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0
}

// The interesting argument of a tool call: the Bash command, the file path, the search pattern...
pub fn tool_arg(input: &Value) -> String {
    let v = ["command", "file_path", "pattern", "skill", "path", "url"].iter().find_map(|k| nn(input.get(*k)));
    js_head(&v.map_or(String::new(), |v| js_string(Some(v))), 300).to_string()
}

// "Bash git status", "Read /a/b.ts": one line per tool call for the events list.
pub fn tool_line(name: &str, input: &Value) -> String {
    js_trim(&format!("{name} {}", tool_arg(input))).to_string()
}

// ---------- git and GitHub (the card's git line and PR line) ----------

// `git status --porcelain=v2 --branch` -> the branch and how many files changed or are untracked.
// Changed counts ordinary (1), renamed (2) and unmerged (u) entries; "(detached)" means no branch.
// Returns { branch, changed, untracked }.
pub fn parse_git_status(out: &str) -> Value {
    let (mut branch, mut changed, mut untracked): (Option<&str>, i64, i64) = (None, 0, 0);
    for line in out.split('\n') {
        if let Some(h) = line.strip_prefix("# branch.head ") {
            branch = (h != "(detached)").then_some(h);
        } else if matches!(line.as_bytes(), [b'1' | b'2' | b'u', b' ', ..]) {
            changed += 1;
        } else if line.starts_with("? ") {
            untracked += 1;
        }
    }
    json!({ "branch": branch, "changed": changed, "untracked": untracked })
}

// `git for-each-ref --format='%(refname) %(symref:short) %(ahead-behind:HEAD)' refs/remotes/origin/HEAD refs/remotes/origin/<branch>`:
//   refs/remotes/origin/HEAD origin/main 5 7       -> 5 behind main, 7 commits on the branch
//   refs/remotes/origin/<branch>  0 1              -> 1 commit not pushed; no such line -> the branch was never pushed
// No origin/HEAD line leaves base, behind and commits null (unknown, not zero).
// Returns { base, behind, commits, push, unpushed }.
pub fn parse_ref_counts(out: &str, branch: Option<&str>) -> Value {
    let branch = branch.filter(|b| !b.is_empty());
    let (mut base, mut behind, mut commits) = (Value::Null, Value::Null, Value::Null);
    let mut unpushed: Option<f64> = None;
    for line in out.split('\n') {
        let parts: Vec<&str> = js_trim(line).split(' ').collect();
        if parts.len() < 3 {
            continue;
        }
        let (theirs, mine) = (js_number(parts[parts.len() - 2]), js_number(parts[parts.len() - 1]));
        if parts[0] == "refs/remotes/origin/HEAD" {
            let b = parts[1].strip_prefix("origin/").unwrap_or(parts[1]);
            (base, behind, commits) = (if b.is_empty() { Value::Null } else { json!(b) }, num(theirs), num(mine));
        } else if branch.is_some_and(|b| parts[0] == format!("refs/remotes/origin/{b}")) {
            unpushed = Some(mine);
        }
    }
    let push = match (branch, unpushed) {
        (None, _) => Value::Null,
        (_, None) => json!("none"),
        (_, Some(u)) => json!(if num_truthy(u) { "unpushed" } else { "pushed" }),
    };
    json!({ "base": base, "behind": behind, "commits": commits, "push": push, "unpushed": num(unpushed.unwrap_or(0.0)) })
}

// "git@github.com:Hostaway/copilot-service.git" or "https://github.com/himmelreich-it/switchboard.git" -> { owner, name }.
pub fn parse_github_remote(url: &str) -> Option<Value> {
    static REMOTE: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"github\.com[:/]([^/{WS}]+)/([^/{WS}]+?)(?:\.git)?/?$")));
    REMOTE.captures(js_trim(url)).map(|m| json!({ "owner": &m[1], "name": &m[2] }))
}

// The gh accounts logged in to github.com. `gh auth status --json hosts` is missing from older gh versions
// (Ubuntu ships 2.46) reject the flag, so their text output is read instead: "Logged in to github.com account X" (2.40+)
// or "Logged in to github.com as X" (older).
pub fn parse_gh_logins(json: Option<&str>, text: &str) -> Vec<String> {
    static LOGIN: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"Logged in to github\.com (?:account|as) ([^{WS}]+)")));
    // JSON that does not read as { hosts: { "github.com": [{ login }] } } falls back to the text, as the TS try/catch did.
    let from_json = || -> Option<Vec<String>> {
        let v: Value = serde_json::from_str(json.filter(|j| !j.is_empty())?).ok()?;
        if v.is_null() {
            return None;
        }
        match nn(at(&v, &["hosts", "github.com"])) {
            None => Some(vec![]),
            Some(Value::Array(hs)) => hs.iter().map(|h| (!h.is_null()).then(|| js_string(h.get("login")))).collect(),
            Some(_) => None,
        }
    };
    if let Some(logins) = from_json() {
        return logins;
    }
    let mut out: Vec<String> = vec![];
    for m in LOGIN.captures_iter(text) {
        if !out.iter().any(|x| x == &m[1]) {
            out.push(m[1].to_string());
        }
    }
    out
}

pub const PR_FIELDS: &str = concat!(
    "fragment F on PullRequest { number url state isDraft mergeable mergeStateStatus reviewDecision baseRefName\n",
    "  reviewThreads(first: 100) { nodes { isResolved } }\n",
    "  commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 100) { nodes {\n",
    "    __typename ... on CheckRun { name status conclusion } ... on StatusContext { context state } } } } } } } }",
);

// One GraphQL query for a whole repository: alias b<i> finds the newest PR whose head is branch $b<i>,
// alias n<i> reads PR number $n<i> (registered with set_pr). Values go in as variables, never into the query text.
// `fields` is PR_FIELDS unless the caller asks for others.
pub fn pr_query(branches: i64, numbers: i64, fields: &str) -> String {
    let mut vars = vec!["$o: String!".to_string(), "$n: String!".to_string()];
    let mut parts: Vec<String> = vec![];
    for i in 0..branches {
        vars.push(format!("$b{i}: String!"));
        parts.push(format!("b{i}: pullRequests(headRefName: $b{i}, first: 1, orderBy: {{field: CREATED_AT, direction: DESC}}) {{ nodes {{ ...F }} }}"));
    }
    for i in 0..numbers {
        vars.push(format!("$n{i}: Int!"));
        parts.push(format!("n{i}: pullRequest(number: $n{i}) {{ ...F }}"));
    }
    format!("query({}) {{ repository(owner: $o, name: $n) {{ {} }} }}\n{fields}", vars.join(", "), parts.join(" "))
}

// One PullRequest node of pr_query() in the shape the card reads. The overall check state is GitHub's rollup;
// the counts and failing names come from the single checks. mergeable UNKNOWN (not computed yet) is neither.
// A Pr is { number, url, state: open|merged|closed, draft, conflicts, mergeable, behind, base,
// review: approved|changes|required|null, threads, checks: { state: failing|running|passing|null, failing, running,
// passed, skipped } }.
pub fn pr_from_node(n: &Value) -> Value {
    let rollup = at(n, &["commits", "nodes"]).and_then(|x| x.get(0)).and_then(|x| at(x, &["commit", "statusCheckRollup"]));
    let state = match rollup.and_then(|r| r.s("state")) {
        Some("FAILURE" | "ERROR") => json!("failing"),
        Some("PENDING" | "EXPECTED") => json!("running"),
        Some("SUCCESS") => json!("passing"),
        _ => Value::Null,
    };
    let (mut failing, mut running, mut passed, mut skipped) = (vec![], 0, 0, 0);
    for c in arr(rollup.and_then(|r| at(r, &["contexts", "nodes"]))) {
        let result = if c.s("__typename") == Some("StatusContext") {
            c.s("state")
        } else if c.s("status") == Some("COMPLETED") {
            c.s("conclusion")
        } else {
            Some("PENDING")
        };
        match result {
            Some("SUCCESS" | "NEUTRAL") => passed += 1,
            Some("SKIPPED") => skipped += 1,
            Some("PENDING" | "EXPECTED") => running += 1,
            _ => failing.push(nn(c.get("name")).or(c.get("context")).cloned().unwrap_or(Value::Null)),
        }
    }
    let review = match n.s("reviewDecision") {
        Some("APPROVED") => json!("approved"),
        Some("CHANGES_REQUESTED") => json!("changes"),
        Some("REVIEW_REQUIRED") => json!("required"),
        _ => Value::Null,
    };
    let mergeable = n.get("mergeable");
    let mut p = Map::new();
    put(&mut p, "number", n.get("number"));
    put(&mut p, "url", n.get("url"));
    p.insert("state".into(), json!(js_string(n.get("state")).to_lowercase()));
    p.insert("draft".into(), json!(n.truthy("isDraft")));
    p.insert("conflicts".into(), json!(mergeable.is_some_and(|m| m == "CONFLICTING")));
    p.insert("mergeable".into(), json!(mergeable.is_some_and(|m| m == "MERGEABLE")));
    p.insert("behind".into(), json!(n.s("mergeStateStatus") == Some("BEHIND")));
    put(&mut p, "base", n.get("baseRefName"));
    p.insert("review".into(), review);
    let threads = arr(nn(at(n, &["reviewThreads", "nodes"]))).iter().filter(|t| !t.truthy("isResolved")).count();
    p.insert("threads".into(), json!(threads));
    p.insert(
        "checks".into(),
        json!({ "state": state, "failing": failing, "running": running, "passed": passed, "skipped": skipped }),
    );
    Value::Object(p)
}

// ---------- worktree scan (Settings › Worktrees) ----------

// The scan needs a PR's state and its commits: a squash merge or a deleted branch leaves no other trace of them.
pub const SCAN_PR_FIELDS: &str = "fragment F on PullRequest { number url state commits(first: 250) { nodes { commit { oid } } } }";

// `git worktree list --porcelain`: blocks split by a blank line, the first one the main checkout.
//   worktree /Users/me/code/worktrees/fix-x
//   HEAD 3f2a…
//   branch refs/heads/fix-x        (or "detached")
//   prunable gitdir file points to non-existent location
// Returns [{ path, head, branch, prunable, locked }].
pub fn parse_worktree_list(out: &str) -> Value {
    static BLANK: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"\n[{WS}]*\n")));
    let blocks = BLANK.split(out).filter(|b| !js_trim(b).is_empty()).map(|block| {
        let (mut path, mut head, mut branch, mut prunable, mut locked) = ("", "", Value::Null, false, false);
        for line in block.split('\n') {
            let (k, v) = line.split_once(' ').unwrap_or((line, ""));
            match k {
                "worktree" => path = v,
                "HEAD" => head = v,
                "branch" => branch = json!(v.strip_prefix("refs/heads/").unwrap_or(v)),
                "prunable" => prunable = true,
                "locked" => locked = true,
                _ => {}
            }
        }
        json!({ "path": path, "head": head, "branch": branch, "prunable": prunable, "locked": locked })
    });
    Value::Array(blocks.collect())
}

// WorktreeFacts: { prunable, locked, branch,
//   changed: string[]     `git status --porcelain` lines: uncommitted and untracked files
//   localOnly: string[]   "<sha> <subject>" of commits on no remote branch and not in the PR
//   pr: { number, url, state: open|merged|closed } | null,
//   todos: [{ id, removed }],
//   branchOnly?: bool      a local branch no worktree has checked out: no TODO can work in it }
// Verdict: { verdict: remove|check|keep, reasons: string[] }

// Keep: a live TODO works in it, or its PR is open. Remove: its PR is merged or closed and nothing in it exists only
// here (no changed file, no commit on no remote). Check: anything else. Keep rows still list what exists only here.
// Also for a branch without a worktree (branchOnly): then only its PR and its commits count.
pub fn worktree_verdict(f: &Value) -> Value {
    if f.truthy("prunable") {
        return json!({ "verdict": "remove", "reasons": ["folder is gone; git still lists it"] });
    }
    let n = |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
    let todos = arr(f.get("todos"));
    let t_id = |t: &Value| format!("T-{}", js_string(t.get("id")));
    let live: Vec<String> = todos.iter().filter(|t| !t.truthy("removed")).map(t_id).collect();
    let (changed, local_only) = (arr(f.get("changed")).len(), arr(f.get("localOnly")).len());
    let mut risks = vec![];
    if changed > 0 {
        risks.push(n(changed, "changed file", "changed files"));
    }
    if local_only > 0 {
        risks.push(n(local_only, "commit on no remote", "commits on no remote"));
    }
    if f.truthy("locked") {
        risks.push("locked".into());
    }
    let pr_v = f.get("pr").filter(|p| truthy(p));
    let pr = match pr_v {
        Some(p) => format!("PR #{} {}", js_string(p.get("number")), js_string(p.get("state"))),
        None => (if f.truthy("branch") { "no PR" } else { "detached HEAD" }).into(),
    };
    let todo = if !live.is_empty() {
        format!("{} live", live.join(", "))
    } else if !todos.is_empty() {
        format!("{} removed", todos.iter().map(t_id).collect::<Vec<_>>().join(", "))
    } else {
        "no TODO".into()
    };
    let mut reasons = vec![pr];
    if !f.truthy("branchOnly") {
        reasons.push(todo);
    }
    let no_risks = risks.is_empty();
    reasons.extend(risks);
    if !live.is_empty() || pr_v.is_some_and(|p| p.s("state") == Some("open")) {
        return json!({ "verdict": "keep", "reasons": reasons });
    }
    json!({ "verdict": if pr_v.is_some() && no_risks { "remove" } else { "check" }, "reasons": reasons })
}

// How one `gh api graphql` call went. gh exits 1 on any GraphQL error but still prints the data, so a bad set_pr number
// (error on path repository.n0) keeps the other PRs. An error on the repository itself, or a rejected token (HTTP 401),
// means this account cannot see it, and the next account is tried.
// Returns { repo } or { error: "no access" | "no network" | "unclear", detail }. `code` None: the process was killed.
pub fn gh_outcome(code: Option<i64>, stdout: &str, stderr: &str) -> Value {
    static NET: LazyLock<Regex> =
        LazyLock::new(|| re(r"(?i-u)dial tcp|error connecting to|internet connection|no such host|i/o timeout|TLS handshake"));
    let j: Value = serde_json::from_str(stdout).unwrap_or(Value::Null);
    let repo_error = arr(j.get("errors")).iter().any(|e| arr(e.get("path")).len() == 1 && e["path"][0] == "repository");
    if repo_error || stderr.contains("HTTP 401") {
        return json!({ "error": "no access", "detail": "" });
    }
    if let Some(repo) = at(&j, &["data", "repository"]).filter(|r| truthy(r)) {
        return json!({ "repo": repo });
    }
    let text = js_trim(if stderr.is_empty() { stdout } else { stderr });
    if NET.is_match(text) {
        return json!({ "error": "no network", "detail": text });
    }
    let first = if text.is_empty() { String::new() } else { format!(" · {}", js_head(text.split('\n').next().unwrap_or(""), 160)) };
    let code = code.map_or("killed".to_string(), |c| c.to_string());
    json!({ "error": "unclear", "detail": format!("exit {code}{first}") })
}

// ---------- files of a worktree or the project folder (items 102, 103) ----------

// `git diff --name-status -z <base>` (the merge base against the working tree, so committed and uncommitted
// changes) plus `git ls-files --others --exclude-standard -z` (untracked): one mark per path, "M" | "A" | "U" | "D".
// A rename marks the new path added and the old one deleted. Returns {path: mark}.
pub fn parse_marks(name_status_z: &str, untracked_z: &str) -> Value {
    let f: Vec<&str> = name_status_z.split('\0').collect();
    // past the end, f[i] is undefined, which becomes the key "undefined"
    let field = |i: usize| f.get(i).copied().unwrap_or("undefined");
    let mut marks = Map::new();
    let mut i = 0;
    while i + 1 < f.len() {
        let st = field(i);
        i += 1;
        let Some(&c) = st.as_bytes().first() else { continue };
        if c == b'R' || c == b'C' {
            let (from, to) = (field(i), field(i + 1));
            i += 2;
            if c == b'R' {
                marks.insert(from.into(), json!("D"));
            }
            marks.insert(to.into(), json!("A"));
            continue;
        }
        let path = field(i);
        i += 1;
        marks.insert(path.into(), json!(match c {
            b'D' => "D",
            b'A' => "A",
            _ => "M",
        }));
    }
    for p in untracked_z.split('\0').filter(|p| !p.is_empty()) {
        marks.insert(p.into(), json!("U"));
    }
    Value::Object(js_order(marks))
}

// ---------- terminals (items 111-118) ----------

// The command that shows a file in the system file manager (item 106): selected in Finder or Explorer. Linux has no
// common way to select a file, so its folder opens. Explorer wants backslashes and the quoted path glued to /select,
// and the caller spawns it with windowsVerbatimArguments: Bun's own quoting ("/select,C:\x y\z") makes Explorer open
// Documents instead. Windows paths cannot contain a quote, so none needs escaping.
pub fn reveal_command(platform: &str, file: &str) -> Vec<String> {
    match platform {
        "darwin" => vec!["open".into(), "-R".into(), file.into()],
        "win32" => vec!["explorer.exe".into(), format!("/select,\"{}\"", file.replace('/', "\\"))],
        _ => vec!["xdg-open".into(), js_dirname(file)],
    }
}

// The command that opens a folder in VS Code. macOS finds the app by its bundle id, so the `code` shell command need
// not be installed. Linux runs `code` from PATH. On Windows `code` is code.cmd, which only cmd.exe runs; /s makes cmd
// drop the outer quotes, and the caller spawns it with windowsVerbatimArguments so it adds no quotes of its own.
// ponytail: a % in a Windows path would expand as a variable; such folder names are rare.
pub fn vscode_command(platform: &str, dir: &str) -> Vec<String> {
    match platform {
        "darwin" => ["open", "-b", "com.microsoft.VSCode", dir].map(String::from).to_vec(),
        "win32" => vec!["cmd.exe".into(), "/d".into(), "/s".into(), "/c".into(), format!("\"code \"{}\"\"", dir.replace('/', "\\"))],
        _ => vec!["code".into(), dir.into()],
    }
}

// The processes below `root` in `ps -A -o pid=,ppid=` output (lines "pid ppid"), children before grandchildren.
pub fn descendants(ps: &str, root: i64) -> Vec<i64> {
    // keyed by the number's bits, with -0 as 0, as a JS Map keys numbers
    let key = |f: f64| (f + 0.0).to_bits();
    let mut kids: HashMap<u64, Vec<f64>> = HashMap::new();
    for line in ps.split('\n') {
        let nums: Vec<f64> = js_trim(line).split(js_ws).filter(|s| !s.is_empty()).map(js_number).collect();
        let pid = nums.first().copied().unwrap_or(0.0);
        if let Some(&ppid) = nums.get(1) {
            if num_truthy(pid) && !ppid.is_nan() {
                kids.entry(key(ppid)).or_default().push(pid);
            }
        }
    }
    let root = root as f64;
    let mut out: Vec<f64> = vec![];
    let mut i = 0;
    let mut parent = root;
    loop {
        for &k in kids.get(&key(parent)).into_iter().flatten() {
            if k != root && !out.contains(&k) {
                out.push(k);
            }
        }
        if i >= out.len() {
            break;
        }
        parent = out[i];
        i += 1;
    }
    out.into_iter().map(|f| f as i64).collect()
}

// What the worker is told about where it runs, between the contract and the topic prompt. The user files TODOs under
// topics; the agent needs a folder, so the intro names both, and where worktrees go (`worktrees`: None outside git).
// `p`: { name, local_path, independent? }; None (or Null) for no topic.
pub fn topic_intro(p: Option<&Value>, worktrees: Option<&str>) -> String {
    let Some(p) = p.filter(|p| truthy(p)) else {
        return "This TODO has no topic: it is thinking only, with no code changes. Your working directory is its artifact folder. Do not create a worktree.".into();
    };
    let head = format!(
        "This TODO belongs to the topic \"{}\". The user knows it by that name; its folder on disk, {}, is your working directory.",
        js_string(p.get("name")),
        js_string(p.get("local_path"))
    );
    let worktrees = worktrees.filter(|w| !w.is_empty());
    let intro = match worktrees {
        Some(w) => format!("{head} The folder is a git repository. Before you start work, pull the latest main: `git pull --ff-only` when the folder is on main, else `git fetch origin main:main`. Worktrees go under {w}, e.g. `git worktree add -b <branch> {w}/<name> main`; the topic prompt may say more."),
        None => format!("{head} The folder is not a git repository: work in it directly, without a worktree, branches or a pull request, and do not run git init."),
    };
    if p.truthy("independent") {
        format!("{intro}\n\n{}", independent_text(worktrees.is_some()))
    } else {
        intro
    }
}

// Independent mode: the user's standing go-ahead to ship work in this topic without them. It overrides the contract's
// "outward actions need an explicit message" for push, pull request and merge only; Jira, Slack and the rest still ask.
fn independent_text(git: bool) -> String {
    let mut lines = vec!["Independent mode is on for this topic. The user turned it on so work here ships without them. It is their standing go-ahead for the steps below, also in a TODO another TODO's agent created, and it is the confirmation that the contract above and your own instructions ask for before these steps."];
    if git {
        lines.push("- Ship your change yourself once you trust it: the tests pass, you read the whole diff, and it does what the TODO asks. Commit, push the branch and open a pull request. When the repository has checks, wait for them (`gh pr checks --watch`; \"no checks reported\" means there are none) and fix a failing one, or ask. Then squash-merge it with `gh pr merge <number> --squash` and pull the latest main. A merge the repository refuses (branch protection, review required): ask.");
    }
    lines.push("- On a design question with a recommended option you are confident in, pick it and go on. Post one line naming the choice and why. Not confident: ask, as usual.");
    lines.push("- Still ask before anything destructive or hard to undo (force-push, history rewrite, deleting data) and before anything outside this topic's repository (Jira, Slack, other repositories).");
    lines.join("\n")
}

// Item 114: what a Claude terminal reads first, written to <artifact folder>/terminal/context.md at each launch.
// `c`: { title, project, status, first, worktree, branch, git, pr, sessions: [{ no, session_id, started_at, ended_at }],
// messages: [{ author, body, at, session_no }], files: string[] }
pub fn context_file(c: &Value) -> String {
    let s = |k: &str| js_string(c.get(k));
    let status = if c.s("status") == Some("not_started") { "not started (in the backlog)".into() } else { s("status") };
    let mut out: Vec<String> =
        vec![format!("# Switchboard TODO: {}", s("title")), "".into(), format!("- Topic: {}", s("project")), format!("- Status: {status}")];
    for (k, label) in [("worktree", "Worktree"), ("branch", "Branch"), ("git", "Git"), ("pr", "Pull request")] {
        if c.truthy(k) {
            out.push(format!("- {label}: {}", s(k)));
        }
    }
    out.extend(["".into(), "## First instruction".into(), "".into(), s("first"), "".into(), "## Note".into(), "".into()]);
    out.push("The TODO's own agent may be working in the same folder or worktree at the same time. Check with the user before you commit, switch branches or rewrite history.".into());
    out.push("".into());
    let files = arr(c.get("files"));
    if !files.is_empty() {
        out.extend(["## Files".into(), "".into()]);
        out.extend(files.iter().map(|f| format!("- {}", js_string(Some(f)))));
        out.push("".into());
    }
    let sessions = arr(c.get("sessions"));
    if !sessions.is_empty() {
        out.extend(["## Sessions".into(), "".into()]);
        out.extend(sessions.iter().map(|x| {
            let end = if x.truthy("ended_at") { format!(" to {}", js_string(x.get("ended_at"))) } else { ", live".into() };
            format!(
                "- Session {}: {} ({}{end}). Its full transcript is under ~/.claude/projects/.",
                js_string(x.get("no")),
                js_string(x.get("session_id")),
                js_string(x.get("started_at"))
            )
        }));
        out.push("".into());
    }
    out.extend(["## Chat".into(), "".into()]);
    // Some(Null) is the one group of all messages; None is an undefined session number.
    let groups: Vec<Option<&Value>> = if sessions.is_empty() { vec![Some(&Value::Null)] } else { sessions.iter().map(|x| x.get("no")).collect() };
    let last_no = sessions.last().and_then(|x| x.get("no"));
    let eq = |a: Option<&Value>, b: Option<&Value>| match (a, b) {
        (Some(a), Some(b)) => same(a, b),
        (a, b) => a.is_none() && b.is_none(),
    };
    for no in groups {
        let all = matches!(no, Some(Value::Null));
        if !all && sessions.len() > 1 {
            out.extend([format!("### Session {}", js_string(no)), "".into()]);
        }
        for m in arr(c.get("messages")) {
            if all || eq(nn(m.get("session_no")).or(last_no), no) {
                out.push(format!("**{}** ({}): {}", js_string(m.get("author")), js_string(m.get("at")), js_string(m.get("body"))));
                out.push("".into());
            }
        }
    }
    out.join("\n")
}

// ---------- schedules (item 144) ----------

// Days 0 = Sunday … 6 = Saturday; times "HH:MM" in the machine's local time. Either fixed `times`, or `every` N minutes
// from `from` to `to` (both included; without them the whole day). `prompt` null: the first instruction is the prompt.
// A Schedule is { days, times?, every?, from?, to?, prompt, paused }.
static HM: LazyLock<Regex> = LazyLock::new(|| re(r"^([01][0-9]|2[0-3]):[0-5][0-9]$"));
fn to_min(hm: &str) -> i64 {
    (js_number(js_head(hm, 2)) * 60.0 + js_number(js_from(hm, 3))) as i64
}
const DAY: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

// A schedule from a request body or a tool call, or the error the user or agent reads.
pub fn schedule_in(v: &Value) -> Result<Value, String> {
    if !v.is_object() {
        return Err("schedule must be an object".into());
    }
    let mut days: Vec<i64> = vec![];
    for d in arr(v.get("days")) {
        if let Some(f) = d.as_f64().filter(|f| f.fract() == 0.0 && (0.0..=6.0).contains(f)) {
            if !days.contains(&(f as i64)) {
                days.push(f as i64);
            }
        }
    }
    days.sort();
    if days.is_empty() {
        return Err("Pick at least one day (days: 0 = Sunday … 6 = Saturday).".into());
    }
    let prompt = v.s("prompt").map(js_trim).filter(|p| !p.is_empty());
    let mut base = Map::new();
    base.insert("days".into(), json!(days));
    base.insert("prompt".into(), json!(prompt));
    base.insert("paused".into(), json!(v.get("paused") == Some(&Value::Bool(true))));
    if nn(v.get("every")).is_some() {
        let every = to_number(v.get("every"));
        if every.fract() != 0.0 || !(1.0..=1440.0).contains(&every) {
            return Err("every is minutes between runs, 1 to 1440.".into());
        }
        // PORT: a non-string from/to that passes the check (["08:00"]) is kept as its string form; JS kept the value.
        let from = v.get("from").filter(|x| truthy(x)).map(|x| js_string(Some(x)));
        let to = v.get("to").filter(|x| truthy(x)).map(|x| js_string(Some(x)));
        if [&from, &to].iter().any(|x| x.as_ref().is_some_and(|x| !HM.is_match(x))) {
            return Err("from and to are times like 08:00.".into());
        }
        if let (Some(f), Some(t)) = (&from, &to) {
            if to_min(f) > to_min(t) {
                return Err("from must come before to.".into());
            }
        }
        base.insert("every".into(), num(every));
        if let Some(f) = from {
            base.insert("from".into(), json!(f));
        }
        if let Some(t) = to {
            base.insert("to".into(), json!(t));
        }
        return Ok(Value::Object(base));
    }
    let mut times: Vec<String> = vec![];
    for x in arr(v.get("times")) {
        let Some(t) = x.as_str().filter(|t| HM.is_match(t)) else { return Err("Give times like 09:00, or every N minutes.".into()) };
        if !times.iter().any(|y| y == t) {
            times.push(t.into());
        }
    }
    if times.is_empty() {
        return Err("Give times like 09:00, or every N minutes.".into());
    }
    times.sort();
    base.insert("times".into(), json!(times));
    Ok(Value::Object(base))
}

// A day's slots in minutes after midnight.
fn slots_of(s: &Value) -> Vec<i64> {
    if !s.truthy("every") {
        return arr(nn(s.get("times"))).iter().map(|t| to_min(&js_string(Some(t)))).collect();
    }
    let every = to_number(s.get("every")) as i64;
    let start = s.s("from").filter(|f| !f.is_empty()).map_or(0, to_min);
    let end = s.s("to").filter(|t| !t.is_empty()).map_or(1439, to_min);
    let mut out = vec![];
    let mut m = start;
    while m <= end {
        out.push(m);
        m += every;
    }
    out
}

/// A local wall-clock time as JS `new Date(y, m, d, h, min)` reads it: in a fall-back hour the earlier of the two; in a
/// spring-forward gap, with the offset from before the gap, so the slot runs that much later.
fn local_wall(naive: NaiveDateTime) -> DateTime<Local> {
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(t) => t,
        LocalResult::Ambiguous(a, b) => a.min(b),
        LocalResult::None => {
            let before = (1..=48).find_map(|h| Local.from_local_datetime(&(naive - Duration::hours(h))).latest());
            let offset = before.map_or(0, |t| t.offset().local_minus_utc() as i64);
            Local.from_utc_datetime(&(naive - Duration::seconds(offset)))
        }
    }
}

// The first slot later than `after`, or None. Built from date parts, not by adding 24 hours, so a slot stays at its
// wall-clock time across a daylight saving change. A slot in the skipped spring-forward hour runs an hour later.
pub fn next_run(s: &Value, after: DateTime<Local>) -> Option<DateTime<Local>> {
    let slots = slots_of(s);
    let days = arr(s.get("days"));
    for d in 0..8 {
        let day = after.date_naive().checked_add_days(chrono::Days::new(d))?;
        let weekday = day.weekday().num_days_from_sunday() as f64;
        if !days.iter().any(|x| x.as_f64() == Some(weekday)) {
            continue;
        }
        for &m in &slots {
            let at = local_wall(day.and_hms_opt(0, 0, 0)? + Duration::minutes(m));
            if at > after {
                return Some(at);
            }
        }
    }
    None
}

// "Weekdays 09:00", "Mon–Fri 09:00, 14:00", "Daily 09:00", "Mon 08:00", "Every 30 min · 08–18", "Every 2 h": the chip
// text (design v9) and the hand-over's run line.
pub fn schedule_text(s: &Value) -> String {
    if s.truthy("every") {
        let every = to_number(s.get("every"));
        let unit = if num_truthy(every % 60.0) { format!("{} min", num_text(every)) } else { format!("{} h", num_text(every / 60.0)) };
        let window = if s.truthy("from") && s.truthy("to") {
            format!(" · {}–{}", js_head(&js_string(s.get("from")), 2), js_head(&js_string(s.get("to")), 2))
        } else {
            String::new()
        };
        return format!("Every {unit}{window}");
    }
    let times: Vec<String> = arr(nn(s.get("times"))).iter().map(join_elem).collect();
    let days = arr(s.get("days"));
    let weekdays = days.iter().map(join_elem).collect::<Vec<_>>().join(",") == "1,2,3,4,5";
    let label = if days.len() == 7 {
        "Daily".to_string()
    } else if weekdays {
        (if times.len() > 1 { "Mon–Fri" } else { "Weekdays" }).to_string()
    } else {
        let has = |d: usize| days.iter().any(|x| x.as_f64() == Some(d as f64));
        [1, 2, 3, 4, 5, 6, 0].into_iter().filter(|d| has(*d)).map(|d| DAY[d]).collect::<Vec<_>>().join(", ")
    };
    format!("{label} {}", times.join(", "))
}

// "Tue 13 Oct 09:00", for events and the hand-over.
pub fn slot_text(d: DateTime<Local>) -> String {
    format!("{} {} {} {:02}:{:02}", DAY[d.weekday().num_days_from_sunday() as usize], d.day(), MON[d.month0() as usize], d.hour(), d.minute())
}

// Why a firing does not run (design §3.3), or None when it runs. A routing run or one asking counts as running or waiting.
// `t`: { status, attention, permission, routing }
pub fn skip_reason(t: &Value, busy: bool, queued: i64) -> Option<String> {
    let status = t.s("status");
    let why = if t.truthy("permission") {
        "waiting for approval"
    } else if t.truthy("attention") || status == Some("waiting") {
        "waiting on you"
    } else if busy || status == Some("running") || t.truthy("routing") {
        "still running"
    } else if queued != 0 {
        "messages waiting"
    } else {
        return None;
    };
    Some(why.into())
}

// ---------- schema ----------

// The whole schema plus its migrations. Migrations add, except one: repo routing (801) made todos.project_id
// nullable, and a build from before it fails on a TODO without a repo.
pub fn migrate_db(c: &rusqlite::Connection) -> anyhow::Result<()> {
    c.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS projects (
  id INTEGER PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL UNIQUE, local_path TEXT NOT NULL,
  agent_prompt TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS todos (
  id INTEGER PRIMARY KEY, project_id INTEGER REFERENCES projects(id), title TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('not_started','running','waiting','idle')), attention INTEGER NOT NULL DEFAULT 0,
  session_id TEXT, worktree_path TEXT, pr_number INTEGER, last_tool TEXT, last_read_at TEXT,
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL, removed_at TEXT, removed_by TEXT);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY, todo_id INTEGER,
  author TEXT NOT NULL CHECK (author IN ('user','agent','user via operator','operator','app')), body TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending','queued','delivered')), file_id INTEGER, committed_at TEXT NOT NULL, delivered_at TEXT);
CREATE TABLE IF NOT EXISTS drafts (key TEXT PRIMARY KEY, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS files (id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, path TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS attachments (id INTEGER PRIMARY KEY, message_id INTEGER NOT NULL, name TEXT NOT NULL, path TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY, todo_id INTEGER, kind TEXT NOT NULL,
  actor TEXT NOT NULL CHECK (actor IN ('user','agent','app','operator')), detail TEXT, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS sessions (
  id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, no INTEGER NOT NULL, session_id TEXT NOT NULL,
  started_at TEXT NOT NULL, ended_at TEXT, UNIQUE (todo_id, no));
CREATE TABLE IF NOT EXISTS comments (
  id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, root TEXT NOT NULL, rel TEXT NOT NULL, path TEXT NOT NULL,
  quote TEXT NOT NULL, line INTEGER, body TEXT NOT NULL, reply TEXT,
  state TEXT NOT NULL CHECK (state IN ('open','replied','resolved')), created_at TEXT NOT NULL, replied_at TEXT);
CREATE INDEX IF NOT EXISTS messages_todo ON messages (todo_id, state);
CREATE INDEX IF NOT EXISTS comments_todo ON comments (todo_id);
CREATE INDEX IF NOT EXISTS events_todo ON events (todo_id);
CREATE INDEX IF NOT EXISTS attachments_message ON attachments (message_id);
"#,
    )?;
    let add_column = |table: &str, col: &str, ty: &str| -> rusqlite::Result<bool> {
        let sql = format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?");
        if c.query_row(&sql, [col], |_| Ok(())).optional()?.is_some() {
            return Ok(false);
        }
        c.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {ty}"))?;
        Ok(true)
    };
    add_column("todos", "pr_number", "INTEGER")?; // databases made before set_pr existed
    add_column("todos", "ctx", "TEXT")?; // context meter of the live session (JSON, see ctx_step)
    add_column("todos", "plan", "TEXT")?; // the agent's plan in its last turn (JSON entries), for the card
    add_column("messages", "session_no", "INTEGER")?;
    add_column("events", "session_no", "INTEGER")?;
    add_column("events", "meta", "TEXT")?; // a result's duration and tool count (JSON)
    add_column("files", "session_no", "INTEGER")?;
    add_column("files", "by_user", "INTEGER")?; // 1: a note the user made with New note; editable while the agent runs
    add_column("projects", "mode", "TEXT")?; // permission mode for the project's agents (null: auto), P3
    add_column("todos", "mode", "TEXT")?; // a TODO's own permission mode (null: the project's)
    add_column("todos", "permission", "TEXT")?; // the open permission request (JSON Permission), while the agent waits
    add_column("todos", "attention_by", "TEXT")?; // who raised the attention flag: agent, app or permission
    add_column("todos", "config", "TEXT")?; // the session's config options as the agent last reported them (JSON ConfigOption[]), P4
    add_column("todos", "overrides", "TEXT")?; // the TODO's own config values, {config id: value} (JSON)
    add_column("projects", "defaults", "TEXT")?; // the project's config values per agent, {agent: {config id: value, mode}} (JSON, P5)
    add_column("projects", "default_agent", "TEXT")?; // the agent of the project's new TODOs (null: claude), P5
    add_column("todos", "agent", "TEXT")?; // the agent of the TODO's live session, or of its first one before the start (null: claude)
    add_column("sessions", "agent", "TEXT")?; // the agent each session ran on (null: claude)
    add_column("messages", "via", "TEXT")?; // the Claude terminal an agent message came from ("Claude 2"), P6; null: the TODO's agent
    add_column("todos", "suggestions", "TEXT")?; // the agent's suggested follow-ups (JSON string[]), cleared at each run start
    add_column("todos", "run_started_at", "TEXT")?; // when the current or last run started, for the running timers (v6)
    add_column("todos", "routing", "TEXT")?; // the routing step while it runs or asks the user (JSON Routing), 801
    add_column("todos", "places", "TEXT")?; // worktree, branch and PR per topic the TODO left (JSON {project id: Place}), Change topic
    // When the TODO's topic (or "no topic") was decided. A TODO made before routing got its project from the user.
    if add_column("todos", "routed_at", "TEXT")? {
        c.execute("UPDATE todos SET routed_at = created_at", [])?;
    }
    add_column("projects", "profile", "TEXT")?; // what lives in the repo, written by a scan or the user, for routing
    add_column("projects", "profile_at", "TEXT")?; // when the last scan wrote the profile
    add_column("projects", "profile_edited", "INTEGER")?; // 1 once the user edited the profile; a Rescan asks first
    add_column("projects", "independent", "INTEGER")?; // 1: Independent mode, the agent may push, open and squash-merge its PR
    add_column("todos", "schedule", "TEXT")?; // the repeat rule (JSON Schedule), item 144; null: not scheduled
    add_column("todos", "next_run_at", "TEXT")?; // the next slot; null when paused or not scheduled
    add_column("todos", "parent_id", "INTEGER")?; // the TODO whose agent created this one (item 145)
    add_column("todos", "key", "TEXT")?; // create_todo's dedupe key, unique below
    add_column("todos", "auto_effort", "TEXT")?; // the effort the router or the agent picked, {value, reason} (JSON); null: medium
    // A TODO has no repo before routing, or none at all: project_id becomes nullable. SQLite cannot drop NOT NULL, so the
    // table is rebuilt from its own CREATE statement. It has no triggers; its one index (todos_key) is made after this.
    let not_null: Option<rusqlite::types::Value> = c
        .query_row("SELECT \"notnull\" AS n FROM pragma_table_info('todos') WHERE name = 'project_id'", [], |r| r.get(0))
        .optional()?;
    if not_null.is_some_and(|n| !matches!(n, rusqlite::types::Value::Null | rusqlite::types::Value::Integer(0))) {
        static TABLE: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i-u)^CREATE TABLE "?todos"?"#));
        static COLUMN: LazyLock<Regex> = LazyLock::new(|| {
            re(&format!(r"(?i-u:\bproject_id)([{WS}]+)(?i-u:INTEGER)[{WS}]+(?i-u:NOT)[{WS}]+(?i-u:NULL)"))
        });
        let sql: String = c.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'todos'", [], |r| r.get(0))?;
        let tx = c.unchecked_transaction()?;
        let sql = TABLE.replace(&sql, "CREATE TABLE todos_new");
        tx.execute_batch(&COLUMN.replace(&sql, "project_id${1}INTEGER"))?;
        tx.execute_batch("INSERT INTO todos_new SELECT * FROM todos; DROP TABLE todos; ALTER TABLE todos_new RENAME TO todos;")?;
        tx.commit()?;
    }
    // Item 145: a removed TODO keeps its key, so it blocks re-creation too.
    c.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS todos_key ON todos (key) WHERE key IS NOT NULL")?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS agents (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, command TEXT NOT NULL, args TEXT NOT NULL DEFAULT '[]', env TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL)",
    )?;
    // P4 kept the defaults flat and the mode in projects.mode, both Claude's: they move under "claude".
    let projects: Vec<(i64, Option<String>, Option<String>)> = c
        .prepare("SELECT id, mode, defaults FROM projects WHERE mode IS NOT NULL OR defaults IS NOT NULL")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    for (id, mode, defaults) in projects {
        let d: Value = match defaults.as_deref().filter(|d| !d.is_empty()) {
            Some(d) => serde_json::from_str(d)?,
            None => json!({}),
        };
        let is_obj = |v: &Value| v.is_object() || v.is_array();
        let nested = match &d {
            Value::Object(m) => m.values().any(is_obj),
            Value::Array(a) => a.iter().any(is_obj),
            _ => false,
        };
        let mode = mode.filter(|m| !m.is_empty());
        if nested && mode.is_none() {
            continue;
        }
        let keys = |v: &Value| match v {
            Value::Object(m) => m.len(),
            Value::Array(a) => a.len(),
            Value::String(s) => js_len(s),
            _ => 0,
        };
        let mut all = if nested {
            d
        } else if keys(&d) > 0 {
            json!({ "claude": d })
        } else {
            json!({})
        };
        if let Some(mode) = mode {
            let mut claude = js_spread(all.get("claude").unwrap_or(&Value::Null));
            claude.insert("mode".into(), json!(mode));
            // On an array, JS sets a property JSON.stringify leaves out: the array stays as it was.
            if let Value::Object(o) = &mut all {
                o.insert("claude".into(), Value::Object(js_order(claude)));
            }
        }
        let text = if matches!(all, Value::Object(_) | Value::Array(_)) && keys(&all) > 0 { Some(stringify(&all)) } else { None };
        c.execute("UPDATE projects SET defaults = ?, mode = NULL WHERE id = ?", rusqlite::params![text, id])?;
    }
    // A started TODO without a sessions row was one session, which becomes session 1: every TODO of a database from
    // before sessions, and one started by an older build on this database. Runs on every start, so it catches both.
    let tx = c.unchecked_transaction()?;
    let ids: Vec<i64> = tx
        .prepare("SELECT id FROM todos t WHERE session_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM sessions s WHERE s.todo_id = t.id)")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for id in ids {
        tx.execute(
            "INSERT INTO sessions (todo_id, no, session_id, started_at)
        SELECT id, 1, session_id, COALESCE((SELECT MIN(e.at) FROM events e WHERE e.todo_id = t.id AND e.kind = 'launch'), created_at) FROM todos t WHERE id = ?",
            [id],
        )?;
        let start: rusqlite::types::Value = tx.query_row("SELECT started_at FROM sessions WHERE todo_id = ?", [id], |r| r.get(0))?;
        tx.execute("UPDATE messages SET session_no = 1 WHERE todo_id = ? AND state = 'delivered' AND session_no IS NULL", [id])?;
        tx.execute("UPDATE events SET session_no = 1 WHERE todo_id = ? AND at >= ? AND session_no IS NULL", rusqlite::params![id, start])?;
        tx.execute("UPDATE files SET session_no = 1 WHERE todo_id = ? AND session_no IS NULL", [id])?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    /// Bun's `toMatchObject`: every field of `expected` is in `actual` (arrays element by element, same length).
    fn matches(actual: &Value, expected: &Value) -> bool {
        match (actual, expected) {
            (Value::Object(a), Value::Object(e)) => e.iter().all(|(k, v)| a.get(k).is_some_and(|x| matches(x, v))),
            (Value::Array(a), Value::Array(e)) => a.len() == e.len() && a.iter().zip(e).all(|(x, y)| matches(x, y)),
            _ => same(actual, expected),
        }
    }
    /// bun:sqlite's `.all()`: rows as objects keyed by column.
    fn rows(c: &rusqlite::Connection, sql: &str) -> Vec<Value> {
        let mut st = c.prepare(sql).unwrap();
        let cols: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
        st.query_map([], |r| {
            let mut o = Map::new();
            for (i, k) in cols.iter().enumerate() {
                let v = match r.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => json!(n),
                    rusqlite::types::ValueRef::Real(f) => json!(f),
                    rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    rusqlite::types::ValueRef::Blob(_) => Value::Null,
                };
                o.insert(k.clone(), v);
            }
            Ok(Value::Object(o))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    #[test]
    fn slugify_and_unique_slug() {
        assert_eq!(slugify("Billing API!"), "billing-api");
        assert_eq!(slugify("  Café / Web  "), "cafe-web");
        assert_eq!(slugify("!!!"), "project");
        assert_eq!(unique_slug("api", &HashSet::new()), "api");
        assert_eq!(unique_slug("api", &HashSet::from(["api".to_string(), "api-2".to_string()])), "api-3");
    }

    #[test]
    fn cut_reason_() {
        // Real errors from the events table.
        assert_eq!(cut_reason("Internal error: API Error: Your computer went to sleep mid-response. The response above may be incomplete.").as_deref(), Some("the computer slept"));
        assert_eq!(cut_reason("Internal error: Request timed out").as_deref(), Some("the request timed out"));
        assert_eq!(cut_reason("Internal error: API Error: 502 Upstream service error. This is a server-side issue").as_deref(), Some("an API server error"));
        assert_eq!(cut_reason("Internal error: You've hit your session limit · resets 5:20pm (Europe/Amsterdam)"), None);
        assert_eq!(cut_reason("Authentication required"), None);
        assert_eq!(cut_reason("Internal error: Claude ended the turn without returning results for tool calls: srvtoolu_01"), None);
        assert_eq!(cut_reason("API Error: 400 invalid_request_error"), None);
    }

    #[test]
    fn cut_to_two_lines_() {
        assert_eq!(cut_to_two_lines("  one\n\ntwo  "), "one\ntwo");
        assert_eq!(cut_to_two_lines("one\ntwo\nthree"), "one\ntwo…");
        let long = cut_to_two_lines(&"x".repeat(500));
        assert_eq!(js_len(&long), 280);
        assert!(long.ends_with('…'));
        assert_eq!(line_count("a\n\nb\n"), 2);
    }

    #[test]
    fn delivery_prompt_() {
        assert_eq!(delivery_prompt(None, &strings(&["fix it", " also tests "]), ""), "fix it\n\nalso tests");
        assert_eq!(delivery_prompt(Some("/w/feat"), &strings(&["go"]), ""), "Your worktree: /w/feat. Work there.\n\ngo");
        assert_eq!(delivery_prompt(None, &[], ""), "continue");
        assert_eq!(delivery_prompt(Some("/w/feat"), &strings(&["", " "]), ""), "Your worktree: /w/feat. Work there.\n\ncontinue");
    }

    #[test]
    fn with_attachments_and_safe_file_name() {
        assert_eq!(with_attachments("fix it", &[]), "fix it");
        assert_eq!(
            with_attachments(" cut off ", &strings(&["/u/1-1-a.png", "/u/1-2-b.pdf"])),
            "cut off\n\nAttached files (open with Read):\n/u/1-1-a.png\n/u/1-2-b.pdf"
        );
        assert_eq!(with_attachments("", &strings(&["/u/1-1-a.png"])), "Attached files (open with Read):\n/u/1-1-a.png");
        assert_eq!(safe_file_name("../My Shot (1).png"), "My-Shot-1-.png");
        assert_eq!(safe_file_name(".."), "file");
        assert_eq!(safe_file_name(".env"), "env");
        assert_eq!(safe_file_name("a/b\\c.txt"), if cfg!(windows) { "c.txt" } else { "b-c.txt" }); // a backslash separates on Windows only
        assert_eq!(safe_file_name(&("x".repeat(300) + ".png")), "x".repeat(116) + ".png");
    }

    #[test]
    fn at_tool_boundary_a_message_is_steered_only_when_the_whole_batch_of_tool_calls_has_ended() {
        let b = |v: &[&str]| v.iter().map(|s| Some(s.to_string())).collect::<Vec<_>>();
        assert!(at_tool_boundary(&b(&["completed"])));
        assert!(at_tool_boundary(&b(&["completed", "failed"])));
        assert!(!at_tool_boundary(&b(&["completed", "in_progress"]))); // a parallel command still runs
        assert!(!at_tool_boundary(&b(&["completed", "pending"])));
        assert!(!at_tool_boundary(&[]));
    }

    #[test]
    fn delivery_action_and_status_after_run() {
        assert_eq!(delivery_action("not_started", false), "pending");
        assert_eq!(delivery_action("running", true), "queue");
        assert_eq!(delivery_action("running", false), "send"); // only waiting for background tasks
        assert_eq!(delivery_action("idle", false), "send");
        assert_eq!(delivery_action("waiting", false), "send");
        assert_eq!(status_after_run(true), "waiting");
        assert_eq!(status_after_run(false), "idle");
    }

    #[test]
    fn is_inside_() {
        assert!(is_inside("/h/data/p/1/plan.md", "/h/data/p/1"));
        assert!(is_inside("/h/data/p/1/sub/x.md", "/h/data/p/1"));
        assert!(!is_inside("/h/data/p/1", "/h/data/p/1"));
        assert!(!is_inside("/h/data/p/12/x.md", "/h/data/p/1"));
        assert!(!is_inside("/h/data/p/1/../2/x.md", "/h/data/p/1"));
    }

    #[test]
    fn tool_arg_and_tool_line() {
        assert_eq!(tool_arg(&json!({ "command": "git push --force origin main", "description": "x" })), "git push --force origin main");
        assert_eq!(tool_line("Bash", &json!({ "command": "git status" })), "Bash git status");
        assert_eq!(tool_line("mcp__app__set_title", &json!({ "title": "x" })), "mcp__app__set_title");
    }

    #[test]
    fn parse_git_status_and_parse_ref_counts() {
        // AI-999-finance-luna: two modified files and one untracked file
        let status = "# branch.oid e3e\n# branch.head feature/AI-999\n# branch.upstream origin/feature/AI-999\n# branch.ab +0 -0\n".to_string()
            + "1 .M N... 100644 100644 100644 e3 e3 a.py\n1 .M N... 100644 100644 100644 4b 4b b.py\n? docs/plan.md";
        assert_eq!(parse_git_status(&status), json!({ "branch": "feature/AI-999", "changed": 2, "untracked": 1 }));
        assert_eq!(parse_git_status("# branch.head (detached)")["branch"], Value::Null);
        // AI-1026: tracks origin/main (branch.ab +7 -5) but was never pushed: no origin/<branch> line
        assert_eq!(
            parse_ref_counts("refs/remotes/origin/HEAD origin/main 5 7\n", Some("bugfix/AI-1026")),
            json!({ "base": "main", "behind": 5, "commits": 7, "push": "none", "unpushed": 0 })
        );
        // renovate: pushed, then one more commit
        assert_eq!(
            parse_ref_counts("refs/remotes/origin/HEAD origin/main 2 3\nrefs/remotes/origin/renovate  0 1\n", Some("renovate")),
            json!({ "base": "main", "behind": 2, "commits": 3, "push": "unpushed", "unpushed": 1 })
        );
        assert_eq!(
            parse_ref_counts("refs/remotes/origin/x  0 0", Some("x")),
            json!({ "base": null, "behind": null, "commits": null, "push": "pushed", "unpushed": 0 })
        );
    }

    #[test]
    fn parse_gh_logins_() {
        let j = json!({ "hosts": { "github.com": [{ "login": "a" }, { "login": "b" }] } }).to_string();
        assert_eq!(parse_gh_logins(Some(&j), ""), strings(&["a", "b"]));
        assert_eq!(
            parse_gh_logins(None, "github.com\n  ✓ Logged in to github.com account himmelreich-it (keyring)\n  - Active account: true\n  ✓ Logged in to github.com account work-acct (keyring)"),
            strings(&["himmelreich-it", "work-acct"])
        );
        assert_eq!(parse_gh_logins(None, "github.com\n  ✓ Logged in to github.com as old-user (oauth_token)"), strings(&["old-user"]));
        assert_eq!(parse_gh_logins(None, "You are not logged into any GitHub hosts. Run gh auth login to authenticate."), Vec::<String>::new());
    }

    #[test]
    fn parse_github_remote_() {
        assert_eq!(parse_github_remote("git@github.com:Hostaway/copilot-service.git"), Some(json!({ "owner": "Hostaway", "name": "copilot-service" })));
        assert_eq!(parse_github_remote("https://github.com/himmelreich-it/switchboard.git\n"), Some(json!({ "owner": "himmelreich-it", "name": "switchboard" })));
        assert_eq!(parse_github_remote("https://gitlab.com/a/b.git"), None);
    }

    #[test]
    fn pr_from_node_and_gh_outcome() {
        // PR #522 as GitHub returned it: open, three failing checks, behind main
        let pr = pr_from_node(&json!({
            "number": 522, "url": "https://github.com/Hostaway/copilot-service/pull/522", "state": "OPEN", "isDraft": false, "mergeable": "MERGEABLE",
            "mergeStateStatus": "BEHIND", "reviewDecision": "REVIEW_REQUIRED", "baseRefName": "main", "reviewThreads": { "nodes": [{ "isResolved": true }] },
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "FAILURE", "contexts": { "nodes": [
                { "__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "FAILURE" },
                { "__typename": "CheckRun", "name": "deploy", "status": "COMPLETED", "conclusion": "SKIPPED" },
                { "__typename": "CheckRun", "name": "e2e", "status": "IN_PROGRESS", "conclusion": null },
                { "__typename": "StatusContext", "context": "ci/build", "state": "ERROR" },
                { "__typename": "StatusContext", "context": "codecov", "state": "SUCCESS" },
            ] } } } }] },
        }));
        assert!(matches(&pr, &json!({ "state": "open", "conflicts": false, "mergeable": true, "behind": true, "base": "main", "review": "required", "threads": 0 })));
        assert_eq!(pr["checks"], json!({ "state": "failing", "failing": ["lint", "ci/build"], "running": 1, "passed": 1, "skipped": 1 }));
        // the field order the page gets
        let keys: Vec<&String> = pr.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["number", "url", "state", "draft", "conflicts", "mergeable", "behind", "base", "review", "threads", "checks"]);
        let merged = pr_from_node(&json!({ "number": 1, "state": "MERGED", "mergeable": "UNKNOWN" }));
        assert!(matches(&merged, &json!({ "state": "merged", "conflicts": false, "mergeable": false })));
        assert!(merged.get("url").is_none()); // undefined, so JSON leaves it out

        let not_found = r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND","path":["repository"],"message":"Could not resolve"}]}"#;
        assert_eq!(gh_outcome(Some(1), not_found, "gh: Could not resolve"), json!({ "error": "no access", "detail": "" }));
        assert_eq!(gh_outcome(Some(1), r#"{"message":"Bad credentials"}"#, "gh: Bad credentials (HTTP 401)"), json!({ "error": "no access", "detail": "" }));
        let partial = r#"{"data":{"repository":{"b0":{"nodes":[]},"n0":null}},"errors":[{"type":"NOT_FOUND","path":["repository","n0"]}]}"#;
        assert_eq!(gh_outcome(Some(1), partial, ""), json!({ "repo": { "b0": { "nodes": [] }, "n0": null } }));
        assert!(matches(
            &gh_outcome(Some(1), "", r#"Post "https://api.github.com/graphql": dial tcp 127.0.0.1:9: connect: connection refused"#),
            &json!({ "error": "no network" })
        ));
        assert_eq!(gh_outcome(Some(1), "", "gh: Something went wrong"), json!({ "error": "unclear", "detail": "exit 1 · gh: Something went wrong" }));
    }

    #[test]
    fn token_ok_() {
        let tok = "ab".repeat(32);
        assert!(token_ok(&tok, Some(&tok), None));
        assert!(token_ok(&tok, None, Some(&tok)));
        assert!(!token_ok(&tok, None, None));
        assert!(!token_ok(&tok, Some("ab"), None));
        assert!(!token_ok(&tok, Some(&"cd".repeat(32)), Some(&tok))); // the header wins over the query
    }

    #[test]
    fn hand_over_leaves_out_lines_without_a_value() {
        let full = hand_over(&json!({
            "no": 3, "title": "Implement MCPs (AI-1234)", "worktree": "/w/ai-1234", "pr": 512,
            "last": { "no": 2, "body": "Server done,\nclient stubs left." },
            "files": [{ "path": "/d/plan.md", "session_no": 1 }, { "path": "/d/notes.md", "session_no": null }, { "path": "/d/notes-2.md", "session_no": null, "by_user": true }],
        }));
        assert_eq!(
            full,
            [
                "This TODO continues earlier work (session 3). Title: Implement MCPs (AI-1234). Keep this title.",
                "Your worktree: /w/ai-1234. Work there.",
                "Pull request: #512.",
                "Last message of session 2: Server done, client stubs left.",
                "Files from earlier sessions (open with Read what you need):",
                "/d/plan.md (session 1)",
                "/d/notes.md (added outside a session)",
                "/d/notes-2.md (the user's notes)",
            ]
            .join("\n")
        );
        assert_eq!(
            hand_over(&json!({ "no": 2, "title": "T", "worktree": null, "pr": null, "last": null, "files": [] })),
            "This TODO continues earlier work (session 2). Title: T. Keep this title."
        );
        assert_eq!(
            hand_over(&json!({ "no": 2, "title": "T", "worktree": null, "pr": null, "last": null, "files": [], "work": "/d/work/" })),
            "This TODO continues earlier work (session 2). Title: T. Keep this title.\nEarlier output for the agent itself (logs, raw results, scripts) is in /d/work/."
        );
        // the header names the worktree, so the usual worktree line is not repeated
        assert_eq!(delivery_prompt(Some("/w"), &strings(&["go"]), "HEADER"), "HEADER\n\ngo");
    }

    #[test]
    fn ctx_step_reads_the_context_from_usage_update() {
        let ups = [
            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "hi" } }),
            json!({ "sessionUpdate": "usage_update", "used": 24532, "size": 1000000, "_meta": { "_claude/model": "claude-opus-5-5" } }),
            json!({ "sessionUpdate": "usage_update", "used": 25010, "size": 1000000 }),
        ];
        let c = ups.iter().fold(Value::Null, |c, u| ctx_step(&c, u));
        assert_eq!(c, json!({ "used": 25010, "window": 1000000, "turns": 0, "model": "claude-opus-5-5" }));
        assert_eq!(c.to_string(), r#"{"used":25010,"window":1000000,"turns":0,"model":"claude-opus-5-5"}"#);
        assert_eq!(ctx_step(&Value::Null, &json!({ "sessionUpdate": "plan" })), Value::Null);
        assert_eq!(
            ctx_step(&c, &json!({ "sessionUpdate": "usage_update", "used": 900000, "size": 1000000, "_meta": { "_claude/origin": { "kind": "task-notification" } } })),
            c
        );
    }

    #[test]
    fn background_step_keeps_the_running_background_tasks() {
        let bash = |id: &str| json!({ "sessionUpdate": "tool_call_update", "toolCallId": format!("t{id}"), "_meta": { "claudeCode": { "toolName": "Bash", "toolResponse": { "stdout": "", "backgroundTaskId": id } } } });
        let done = json!({ "sessionUpdate": "usage_update", "used": 1, "_meta": { "_claude/origin": { "kind": "task-notification", "producer": "session-task" } } });
        let stop = |id: &str| json!({ "sessionUpdate": "tool_call_update", "_meta": { "claudeCode": { "toolName": "TaskStop", "toolResponse": { "message": "Successfully stopped task", "task_id": id } } } });
        let mut ids = [bash("a"), bash("b"), bash("a"), bash("c")].iter().fold(vec![], |ids, u| background_step(&ids, u));
        assert_eq!(ids, strings(&["a", "b", "c"]));
        ids = background_step(&ids, &stop("b"));
        assert_eq!(ids, strings(&["a", "c"]));
        assert_eq!(background_step(&ids, &done), ids); // a follow-up does not say which task ended: the level does
        assert_eq!(background_step(&ids, &stop("x")), ids);
        assert_eq!(background_step(&ids, &json!({ "sessionUpdate": "usage_update", "_meta": { "_claude/origin": { "kind": "human" } } })), ids);
    }

    #[test]
    fn background_level_replaces_the_list_with_the_sdks_live_shell_tasks() {
        let level = |tasks: Vec<Value>| json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": tasks });
        let sh = |task_id: &str| json!({ "task_id": task_id, "task_type": "local_bash", "description": "x" });
        // T-14: six servers backgrounded, then ended with a plain `kill`; only the level says so.
        assert_eq!(background_level(&strings(&["a", "b", "c"]), &level(vec![sh("a"), sh("c")])), strings(&["a", "c"]));
        assert_eq!(background_level(&strings(&["a", "b"]), &level(vec![])), Vec::<String>::new());
        // A task the level knows first joins at the end; the order of the others stays.
        assert_eq!(background_level(&strings(&["b"]), &level(vec![sh("a"), sh("b")])), strings(&["b", "a"]));
        // Subagents, monitors and ambient tasks do not count.
        let mut ambient = sh("w");
        ambient["ambient"] = json!(true);
        assert_eq!(
            background_level(&[], &level(vec![sh("a"), json!({ "task_id": "g", "task_type": "local_agent" }), json!({ "task_id": "m", "task_type": "local_monitor" }), ambient])),
            strings(&["a"])
        );
        let ids = strings(&["a"]);
        assert_eq!(background_level(&ids, &level(vec![sh("a")])), ids);
        assert_eq!(background_level(&ids, &json!({ "type": "system", "subtype": "task_notification", "task_id": "a" })), ids);
        assert_eq!(background_level(&ids, &Value::Null), ids);
    }

    #[test]
    fn agent_activity_and_cycle_end_tell_a_cycle_the_agent_runs_on_its_own() {
        let usage = |kind: Option<&str>| match kind {
            Some(kind) => json!({ "sessionUpdate": "usage_update", "used": 1, "_meta": { "_claude/origin": { "kind": kind } } }),
            None => json!({ "sessionUpdate": "usage_update", "used": 1 }),
        };
        assert!(agent_activity(&json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "x" } })));
        assert!(agent_activity(&usage(None)));
        assert!(agent_activity(&usage(Some("task-notification"))));
        assert!(!agent_activity(&usage(Some("human"))));
        assert!(!agent_activity(&json!({ "sessionUpdate": "session_info_update", "title": "t" })));
        assert!(!agent_activity(&json!({ "sessionUpdate": "available_commands_update", "availableCommands": [] })));
        assert!(cycle_end(&usage(Some("task-notification"))));
        assert!(cycle_end(&usage(Some("peer"))));
        assert!(!cycle_end(&usage(Some("human"))));
        assert!(!cycle_end(&usage(Some("channel"))));
        assert!(!cycle_end(&usage(None)));
    }

    #[test]
    fn migrate_db_makes_earlier_todos_session_1_and_runs_twice_without_harm() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        // a database from before sessions
        db.execute_batch(
            "CREATE TABLE todos (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL, title TEXT NOT NULL, status TEXT NOT NULL, attention INTEGER NOT NULL DEFAULT 0,
    session_id TEXT, worktree_path TEXT, last_tool TEXT, last_read_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, removed_at TEXT, removed_by TEXT);
    CREATE TABLE messages (id INTEGER PRIMARY KEY, todo_id INTEGER, author TEXT NOT NULL, body TEXT NOT NULL, state TEXT NOT NULL, file_id INTEGER, committed_at TEXT NOT NULL, delivered_at TEXT);
    CREATE TABLE events (id INTEGER PRIMARY KEY, todo_id INTEGER, kind TEXT NOT NULL, actor TEXT NOT NULL, detail TEXT, at TEXT NOT NULL);
    CREATE TABLE files (id INTEGER PRIMARY KEY, todo_id INTEGER NOT NULL, path TEXT NOT NULL UNIQUE);
    INSERT INTO todos VALUES (1, 1, 'started', 'idle', 0, 'sid-1', NULL, NULL, NULL, 't0', 't9', NULL, NULL);
    INSERT INTO todos VALUES (2, 1, 'idea', 'not_started', 0, NULL, NULL, NULL, NULL, 't0', 't0', NULL, NULL);
    INSERT INTO messages (todo_id, author, body, state, committed_at) VALUES (1, 'user', 'go', 'delivered', 't1'), (1, 'user', 'later', 'queued', 't5'), (2, 'user', 'idea', 'pending', 't0');
    INSERT INTO events (todo_id, kind, actor, at) VALUES (1, 'created', 'user', 't0'), (1, 'launch', 'app', 't2'), (1, 'run ended', 'app', 't3');
    INSERT INTO files (todo_id, path) VALUES (1, '/d/plan.md');",
        )
        .unwrap();
        migrate_db(&db).unwrap();
        migrate_db(&db).unwrap();
        assert_eq!(rows(&db, "SELECT todo_id, no, session_id, started_at FROM sessions"), vec![json!({ "todo_id": 1, "no": 1, "session_id": "sid-1", "started_at": "t2" })]);
        let col = |sql: &str, k: &str| rows(&db, sql).iter().map(|r| r[k].clone()).collect::<Vec<_>>();
        assert_eq!(col("SELECT body, session_no FROM messages ORDER BY id", "session_no"), vec![json!(1), Value::Null, Value::Null]);
        assert_eq!(col("SELECT kind, session_no FROM events ORDER BY id", "session_no"), vec![Value::Null, json!(1), json!(1)]);
        assert_eq!(rows(&db, "SELECT session_no FROM files")[0]["session_no"], json!(1));
        assert!(!rows(&db, "SELECT 1 FROM pragma_table_info('todos') WHERE name = 'ctx'").is_empty());
        // 801: project_id is nullable now, the rows stay, and TODOs from before routing count as routed
        assert_eq!(rows(&db, "SELECT \"notnull\" AS n FROM pragma_table_info('todos') WHERE name = 'project_id'")[0]["n"], json!(0));
        assert_eq!(
            rows(&db, "SELECT id, project_id, title, session_id, routed_at FROM todos ORDER BY id"),
            vec![
                json!({ "id": 1, "project_id": 1, "title": "started", "session_id": "sid-1", "routed_at": "t0" }),
                json!({ "id": 2, "project_id": 1, "title": "idea", "session_id": null, "routed_at": "t0" }),
            ]
        );
        db.execute("INSERT INTO todos (title, status, created_at, updated_at) VALUES ('no repo', 'not_started', 't', 't')", []).unwrap();
    }

    #[test]
    fn parse_marks_() {
        let ns = ["M", "src/a.ts", "A", "src/new.ts", "D", "old.md", "R087", "x/from.ts", "x/to.ts", ""].join("\0");
        assert_eq!(
            parse_marks(&ns, &["notes.txt", "tmp/u.log", ""].join("\0")),
            json!({ "src/a.ts": "M", "src/new.ts": "A", "old.md": "D", "x/from.ts": "D", "x/to.ts": "A", "notes.txt": "U", "tmp/u.log": "U" })
        );
        assert_eq!(parse_marks("", ""), json!({}));
    }

    #[test]
    fn terminal_context_file() {
        let ctx = context_file(&json!({
            "title": "T", "project": "p", "status": "idle", "first": "do it", "worktree": "/w", "branch": "b", "git": "2 commits · pushed", "pr": "#5 open",
            "sessions": [{ "no": 1, "session_id": "s1", "started_at": "a", "ended_at": "b" }, { "no": 2, "session_id": "s2", "started_at": "c", "ended_at": null }],
            "messages": [{ "author": "user", "body": "do it", "at": "a", "session_no": 1 }, { "author": "agent", "body": "done", "at": "b", "session_no": 2 }, { "author": "user", "body": "queued", "at": "d", "session_no": null }],
            "files": ["/d/plan.md"],
        }));
        assert!(ctx.contains("- Worktree: /w"));
        assert!(ctx.contains("### Session 2\n\n**agent** (b): done\n\n**user** (d): queued"));
        assert!(ctx.contains("Check with the user before you commit"));
        assert!(context_file(&json!({ "title": "T", "project": "p", "status": "not_started", "first": "x", "worktree": null, "branch": null, "git": null, "pr": null, "sessions": [], "messages": [], "files": [] }))
            .contains("not started (in the backlog)"));
    }

    #[test]
    fn tool_meta_merges_a_tool_calls_updates_plan_count() {
        let ups = [
            json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Write", "kind": "edit", "status": "pending", "rawInput": {}, "_meta": { "claudeCode": { "toolName": "Write" } } }),
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "title": "Write b.txt", "rawInput": { "file_path": "/r/b.txt", "content": "x" },
                "content": [{ "type": "diff", "path": "/r/b.txt", "oldText": null, "newText": "x" }], "locations": [{ "path": "/r/b.txt" }] }),
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed", "content": [{ "type": "content", "content": { "type": "text", "text": "done" } }] }),
        ];
        let m = ups.iter().fold(Value::Null, |m, u| tool_meta(Some(&m), u));
        assert!(matches(
            &m,
            &json!({ "name": "Write", "title": "Write b.txt", "kind": "edit", "status": "completed", "input": { "file_path": "/r/b.txt" }, "output": "done",
                "locations": [{ "path": "/r/b.txt" }], "diffs": [{ "path": "/r/b.txt", "oldText": null, "newText": "x" }] })
        ));
        assert!(js_len(tool_meta(None, &json!({ "toolCallId": "t2", "rawOutput": "x".repeat(9000), "status": "completed" }))["output"].as_str().unwrap()) < 8100);
        assert_eq!(plan_count(&Value::Null), None);
        assert_eq!(plan_count(&json!([{ "content": "a", "status": "completed" }, { "content": "b", "status": "pending" }])).as_deref(), Some("1/2"));
    }

    #[test]
    fn permission_of_keeps_what_the_ui_shows_of_a_request_p3() {
        let bash = permission_of(
            &json!({
                "sessionId": "s", "toolCall": { "toolCallId": "t1", "title": "rm -rf build", "kind": "execute", "status": "pending", "rawInput": { "command": "rm -rf build" }, "_meta": { "claudeCode": { "toolName": "Bash" } } },
                "options": [{ "optionId": "allow-once", "name": "Allow once", "kind": "allow_once" }, { "optionId": "reject", "name": "Reject", "kind": "reject_once" }],
            }),
            "2026-10-07T10:00:00.000Z",
        );
        assert_eq!(
            bash,
            json!({
                "id": "t1", "name": "Bash", "title": "rm -rf build", "kind": "execute", "input": { "command": "rm -rf build" }, "diffs": [], "at": "2026-10-07T10:00:00.000Z",
                "options": [{ "optionId": "allow-once", "name": "Allow once", "kind": "allow_once" }, { "optionId": "reject", "name": "Reject", "kind": "reject_once" }],
            })
        );
        // the field order of the stored JSON
        assert!(bash.to_string().starts_with(r#"{"id":"t1","name":"Bash","title":"rm -rf build","kind":"execute","input":"#));
        let edit = permission_of(
            &json!({
                "sessionId": "s", "toolCall": { "toolCallId": "t2", "title": "Write a.txt", "kind": "edit", "content": [{ "type": "diff", "path": "/r/a.txt", "oldText": null, "newText": "x".repeat(300_000) }] },
                "options": [{ "optionId": "allow-with-updates", "name": "Allow all edits during this session", "kind": "allow_always" }],
            }),
            "2026-10-07T10:00:00.000Z",
        );
        assert_eq!(edit["name"], "Write a.txt"); // no Claude tool name in _meta: the title stands in
        assert!(js_len(edit["diffs"][0]["newText"].as_str().unwrap()) < 200_100);
        assert_eq!(edit["options"][0]["kind"], "allow_always");
        // A bare tool call still gives something to show.
        assert!(matches(
            &permission_of(&json!({ "sessionId": "s", "toolCall": { "toolCallId": "t3" }, "options": [] }), "now"),
            &json!({ "id": "t3", "name": "tool", "title": "", "kind": "other", "options": [] })
        ));
    }

    #[test]
    fn lane_of_attention_after_answer_and_modes_p3() {
        let perm = json!({ "id": "t", "name": "Bash", "title": "", "kind": "execute", "input": {}, "diffs": [], "options": [], "at": "" });
        assert_eq!(lane_of(&json!({ "status": "waiting", "permission": null })), "waiting");
        assert_eq!(lane_of(&json!({ "status": "running", "permission": perm })), "waiting"); // a question the agent waits on needs the user now
        assert_eq!(lane_of(&json!({ "status": "running", "permission": null })), "running");
        assert_eq!(lane_of(&json!({ "status": "idle", "permission": perm })), "idle"); // stale column after a crash: the status wins
        assert_eq!(lane_of(&json!({ "status": "not_started", "permission": null })), "not_started");
        // T-42: the agent's report asks the user while its Monitor still runs. Only the agent's own flag moves it.
        let bg = |attention_by: Value, background_wait: bool| json!({ "status": "running", "permission": null, "background_wait": background_wait, "attention_by": attention_by });
        assert_eq!(lane_of(&bg(json!("agent"), true)), "waiting");
        assert_eq!(lane_of(&bg(json!("app"), true)), "running"); // "background task finished" on a progress follow-up
        assert_eq!(lane_of(&bg(Value::Null, true)), "running");
        assert_eq!(lane_of(&bg(json!("agent"), false)), "running"); // mid-turn: the agent works on
        // Answering clears the flag only when the request raised it.
        assert_eq!(attention_after_answer(Some("permission")), 0);
        assert_eq!(attention_after_answer(Some("agent")), 1);
        assert_eq!(attention_after_answer(Some("app")), 1);
        assert_eq!(attention_after_answer(None), 1);
        assert_eq!(MODES[0], "auto");
        assert!(MODES.contains(&"default"));
    }

    #[test]
    fn auto_effort_the_picked_level_medium_by_default_never_one_the_user_set_or_the_agent_lacks() {
        let eff = |values: &[&str]| json!([{ "id": "effort", "name": "Effort", "category": "thought_level", "value": "high", "options": values.iter().map(|v| json!({ "value": v, "name": v })).collect::<Vec<_>>() }]);
        let pair = |a: &str, b: &str| json!([a, b]);
        let all = eff(&["default", "low", "medium", "high", "xhigh", "max"]);
        assert_eq!(auto_effort(&all, &json!({}), None), pair("effort", "medium"));
        assert_eq!(auto_effort(&all, &json!({}), Some("xhigh")), pair("effort", "xhigh"));
        assert_eq!(auto_effort(&all, &json!({}), Some("max")), pair("effort", "medium")); // not an auto level: the default
        assert_eq!(auto_effort(&all, &json!({ "effort": "max" }), Some("low")), Value::Null); // the user's (TODO or topic) wins
        assert_eq!(auto_effort(&eff(&["low", "medium", "high"]), &json!({}), Some("xhigh")), pair("effort", "high")); // next lower offered
        assert_eq!(auto_effort(&eff(&["high"]), &json!({}), Some("low")), Value::Null);
        assert_eq!(auto_effort(&json!([]), &json!({}), Some("high")), Value::Null); // no effort option (Haiku)
    }

    #[test]
    fn config_view_and_config_changes_the_model_first_then_what_the_model_left_p4() {
        let raw = json!([
            { "id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "auto", "options": [{ "value": "auto", "name": "Auto" }] },
            { "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "default", "options": [{ "value": "default", "name": "Default" }, { "value": "haiku", "name": "Haiku 4.5" }] },
            { "id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "medium", "options": [{ "group": "g", "name": "G", "options": [{ "value": "low", "name": "Low" }, { "value": "medium", "name": "Medium" }] }] },
            { "id": "flag", "name": "Flag", "type": "boolean", "currentValue": true },
        ]);
        let v = config_view(&raw);
        assert_eq!(v.as_array().unwrap().iter().map(|o| o["id"].clone()).collect::<Vec<_>>(), [json!("mode"), json!("model"), json!("effort")]);
        assert_eq!(
            v[2],
            json!({ "id": "effort", "name": "Effort", "category": "thought_level", "value": "medium", "options": [{ "value": "low", "name": "Low" }, { "value": "medium", "name": "Medium" }] })
        );
        let wanted = json!({ "model": "haiku", "effort": "low", "mode": "plan", "agent": "x" });
        assert_eq!(config_changes(&v, &wanted, true), json!({ "set": [["model", "haiku"]], "skipped": [] }));
        assert_eq!(config_changes(&v, &wanted, false), json!({ "set": [["effort", "low"]], "skipped": [["agent", "x"]] }));
        // Haiku has no effort option: the value is skipped, not set.
        let haiku = Value::Array(
            v.as_array()
                .unwrap()
                .iter()
                .filter(|o| o["id"] != "effort")
                .map(|o| {
                    let mut o = o.clone();
                    if o["id"] == "model" {
                        o["value"] = json!("haiku");
                    }
                    o
                })
                .collect(),
        );
        assert_eq!(config_changes(&haiku, &wanted, true)["set"], json!([]));
        assert_eq!(config_changes(&haiku, &wanted, false)["skipped"], json!([["effort", "low"], ["agent", "x"]]));
        assert_eq!(config_changes(&v, &json!({ "effort": "max" }), false)["skipped"], json!([["effort", "max"]]));
    }

    #[test]
    fn config_in_keeps_string_values_and_drops_the_mode() {
        assert_eq!(config_in(&json!({ "model": "opus", "effort": "", "mode": "plan", "fast": 1 })), json!({ "model": "opus" }));
        assert_eq!(config_in(&Value::Null), json!({}));
        assert_eq!(config_in(&json!(["x"])), json!({}));
    }

    #[test]
    fn reveal_command_() {
        assert_eq!(reveal_command("darwin", "/a/b c.md"), strings(&["open", "-R", "/a/b c.md"]));
        assert_eq!(reveal_command("win32", "C:/code/a b/c.md"), strings(&["explorer.exe", r#"/select,"C:\code\a b\c.md""#]));
        assert_eq!(reveal_command("linux", "/a/b/c.md"), strings(&["xdg-open", "/a/b"]));
    }

    #[test]
    fn vscode_command_() {
        assert_eq!(vscode_command("darwin", "/a/b c"), strings(&["open", "-b", "com.microsoft.VSCode", "/a/b c"]));
        assert_eq!(vscode_command("win32", "C:/code/a b"), strings(&["cmd.exe", "/d", "/s", "/c", r#""code "C:\code\a b"""#]));
        assert_eq!(vscode_command("linux", "/a/b c"), strings(&["code", "/a/b c"]));
    }

    #[test]
    fn follow_up_error_() {
        assert_eq!(follow_up_error(&strings(&["Approve the PR"])), None);
        assert_eq!(follow_up_error(&strings(&["Approve the PR", "Post the two nits as review comments"])), None);
        assert_eq!(follow_up_error(&[]).as_deref(), Some("Suggest one or two follow-ups."));
        assert_eq!(follow_up_error(&strings(&["a", "b", "c"])).as_deref(), Some("Suggest one or two follow-ups."));
        assert_eq!(follow_up_error(&strings(&["  "])).as_deref(), Some("A follow-up is empty."));
        assert_eq!(follow_up_error(&strings(&["one\ntwo"])).as_deref(), Some("A follow-up is one line."));
        assert_eq!(follow_up_error(&["x".repeat(81)]).as_deref(), Some("A follow-up is at most 80 characters."));
        assert_eq!(follow_up_error(&["x".repeat(80)]), None);
    }

    #[test]
    fn migrate_db_adds_todos_suggestions() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        migrate_db(&db).unwrap();
        assert!(!rows(&db, "SELECT 1 FROM pragma_table_info('todos') WHERE name = 'suggestions'").is_empty());
    }

    #[test]
    fn topic_intro_the_users_topic_the_agents_folder_and_whether_a_worktree_is_possible() {
        let p = json!({ "name": "Notes", "local_path": "/home/u/notes" });
        let (git, plain, none) = (topic_intro(Some(&p), Some("/home/u/worktrees")), topic_intro(Some(&p), None), topic_intro(None, None));
        for s in [&git, &plain] {
            assert!(s.contains("topic \"Notes\""));
            assert!(s.contains("/home/u/notes, is your working directory"));
        }
        assert!(git.contains("is a git repository. Before you start work, pull the latest main"));
        assert!(git.contains("Worktrees go under /home/u/worktrees, e.g. `git worktree add -b <branch> /home/u/worktrees/<name> main`"));
        assert!(plain.contains("not a git repository: work in it directly, without a worktree"));
        assert!(none.contains("has no topic"));
        assert!(none.contains("Do not create a worktree"));
        for s in [&git, &plain] {
            assert!(!s.contains("Independent mode"));
        }
        let mut indep = p.clone();
        indep["independent"] = json!(1);
        let (ind, ind_plain) = (topic_intro(Some(&indep), Some("/home/u/worktrees")), topic_intro(Some(&indep), None));
        assert!(ind.contains("Independent mode is on"));
        assert!(ind.contains("gh pr merge <number> --squash"));
        assert!(ind_plain.contains("Independent mode is on"));
        assert!(!ind_plain.contains("gh pr merge")); // no pull request outside git
        assert!(ind_plain.contains("recommended option"));
    }

    #[test]
    fn schedule_in_schedule_text_and_next_run_item_144_across_a_dst_change() {
        // chrono reads TZ when this thread first asks for local time; no other test reads local time.
        let tz = std::env::var("TZ").ok();
        std::env::set_var("TZ", "Europe/Amsterdam"); // clocks go back on Sun 25 Oct 2026, 03:00 -> 02:00
        let wd = schedule_in(&json!({ "days": [5, 1, 2, 3, 4, 4], "times": ["14:00", "09:00"], "prompt": "  " })).unwrap();
        assert_eq!(wd, json!({ "days": [1, 2, 3, 4, 5], "times": ["09:00", "14:00"], "prompt": null, "paused": false }));
        assert_eq!(schedule_text(&wd), "Mon–Fri 09:00, 14:00");
        let with = |s: &Value, k: &str, v: Value| {
            let mut s = s.clone();
            s[k] = v;
            s
        };
        assert_eq!(schedule_text(&with(&wd, "times", json!(["09:00"]))), "Weekdays 09:00");
        assert_eq!(schedule_text(&with(&with(&wd, "days", json!([0, 1, 2, 3, 4, 5, 6])), "times", json!(["09:00"]))), "Daily 09:00");
        assert_eq!(schedule_text(&with(&with(&wd, "days", json!([6, 1])), "times", json!(["08:00"]))), "Mon, Sat 08:00");
        // Tue 13 Oct 09:00 has passed at 09:00 itself; the next slot is 14:00, then Wed 09:00.
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Local);
        assert_eq!(slot_text(next_run(&wd, at("2026-10-13T09:00:00+02:00")).unwrap()), "Tue 13 Oct 14:00");
        assert_eq!(slot_text(next_run(&wd, at("2026-10-13T14:00:00+02:00")).unwrap()), "Wed 14 Oct 09:00");
        assert_eq!(slot_text(next_run(&wd, at("2026-10-16T15:00:00+02:00")).unwrap()), "Mon 19 Oct 09:00"); // Fri evening: skips the weekend
        // Sat 24 Oct 09:00 CEST, then Sun 25 Oct 09:00 CET: one day apart is 25 hours, the wall clock stays 09:00.
        let daily = schedule_in(&json!({ "days": [0, 1, 2, 3, 4, 5, 6], "times": ["09:00"] })).unwrap();
        let sun = next_run(&daily, at("2026-10-24T09:00:00+02:00")).unwrap();
        assert_eq!(slot_text(sun), "Sun 25 Oct 09:00");
        assert_eq!(crate::util::iso(sun.with_timezone(&chrono::Utc)), "2026-10-25T08:00:00.000Z");
        let every = schedule_in(&json!({ "days": [1, 2, 3, 4, 5], "every": 30, "from": "08:00", "to": "18:00" })).unwrap();
        assert_eq!(schedule_text(&every), "Every 30 min · 08–18");
        assert_eq!(slot_text(next_run(&every, at("2026-10-13T11:31:00+02:00")).unwrap()), "Tue 13 Oct 12:00");
        assert_eq!(slot_text(next_run(&every, at("2026-10-13T18:00:00+02:00")).unwrap()), "Wed 14 Oct 08:00"); // to is included, then the window ends
        assert_eq!(schedule_text(&schedule_in(&json!({ "days": [1], "every": 120 })).unwrap()), "Every 2 h");
        assert_eq!(next_run(&with(&daily, "days", json!([])), Local::now()), None);
        // Beyond lib.test.ts: a slot in the spring-forward gap (Sun 29 Mar 2026, 02:00 -> 03:00) runs an hour later.
        let gap = schedule_in(&json!({ "days": [0], "times": ["02:30"] })).unwrap();
        assert_eq!(slot_text(next_run(&gap, at("2026-03-28T12:00:00+01:00")).unwrap()), "Sun 29 Mar 03:30");
        match tz {
            Some(tz) => std::env::set_var("TZ", tz),
            None => std::env::remove_var("TZ"),
        }
        assert!(schedule_in(&json!({ "days": [], "times": ["09:00"] })).unwrap_err().contains("day"));
        assert!(schedule_in(&json!({ "days": [1], "times": ["9:00"] })).unwrap_err().contains("09:00"));
        assert!(schedule_in(&json!({ "days": [1], "every": 0 })).unwrap_err().contains("1 to 1440"));
        assert!(schedule_in(&json!({ "days": [1], "every": 30, "from": "18:00", "to": "08:00" })).unwrap_err().contains("before"));
    }

    #[test]
    fn lane_of_puts_idle_and_not_started_scheduled_todos_in_scheduled_skip_reason_item_144() {
        let sch = json!({ "days": [1], "times": ["09:00"] });
        assert_eq!(lane_of(&json!({ "status": "idle", "permission": null, "schedule": sch })), "scheduled");
        assert_eq!(lane_of(&json!({ "status": "not_started", "permission": null, "schedule": sch })), "scheduled");
        assert_eq!(lane_of(&json!({ "status": "running", "permission": null, "schedule": sch })), "running");
        assert_eq!(lane_of(&json!({ "status": "waiting", "permission": null, "schedule": sch })), "waiting");
        let t = json!({ "status": "idle", "attention": 0, "permission": null, "routing": null });
        let with = |pairs: &[(&str, Value)]| {
            let mut t = t.clone();
            for (k, v) in pairs {
                t[*k] = v.clone();
            }
            t
        };
        assert_eq!(skip_reason(&t, false, 0), None);
        assert_eq!(skip_reason(&t, true, 0).as_deref(), Some("still running")); // background tasks or a turn
        assert_eq!(skip_reason(&with(&[("routing", json!("{}"))]), false, 0).as_deref(), Some("still running"));
        assert_eq!(skip_reason(&with(&[("attention", json!(1))]), false, 0).as_deref(), Some("waiting on you"));
        assert_eq!(
            skip_reason(&with(&[("status", json!("running")), ("attention", json!(1)), ("permission", json!("{}"))]), true, 0).as_deref(),
            Some("waiting for approval")
        );
        assert_eq!(skip_reason(&t, false, 2).as_deref(), Some("messages waiting"));
    }

    #[test]
    fn migrate_db_create_todo_keys_are_unique_also_over_removed_todos_item_145() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        migrate_db(&db).unwrap();
        migrate_db(&db).unwrap();
        let add = |key: Option<&str>| {
            db.execute("INSERT INTO todos (title, status, created_at, updated_at, key, removed_at) VALUES ('x', 'idle', '', '', ?, 'gone')", [key])
        };
        add(Some("pr:a#1")).unwrap();
        add(None).unwrap();
        add(None).unwrap();
        assert!(add(Some("pr:a#1")).is_err());
        assert_eq!(
            hand_over(&json!({ "no": 7, "title": "T", "worktree": null, "pr": null, "last": null, "files": [], "scheduled": "Weekdays 09:00, this one at Tue 13 Oct 09:00" })),
            "This TODO continues earlier work (session 7). Title: T. Keep this title.\nScheduled run: Weekdays 09:00, this one at Tue 13 Oct 09:00."
        );
    }

    #[test]
    fn descendants_() {
        // 10 -> 11 -> 13, 10 -> 12; 20 is not below 10; a blank line and a header-like line are skipped
        let ps = "  10     1\n   11    10\n   12    10\n   13    11\n   20     1\n\n  PID  PPID\n";
        assert_eq!(descendants(ps, 10), vec![11, 12, 13]);
        assert_eq!(descendants(ps, 13), Vec::<i64>::new());
        assert_eq!(descendants("", 10), Vec::<i64>::new());
    }

    #[test]
    fn background_names_keeps_each_tasks_description_from_the_sdks_level() {
        let level = |tasks: Vec<Value>| json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": tasks });
        let sh = |task_id: &str, description: &str| json!({ "task_id": task_id, "task_type": "local_bash", "description": description });
        let n = background_names(&json!({}), &level(vec![sh("a", "bun test --watch"), sh("b", "")]));
        assert_eq!(n, json!({ "a": "bun test --watch" }));
        assert_eq!(background_names(&n, &level(vec![sh("a", "bun test --watch")])), n); // nothing new: same object
        // subagents and ambient tasks are not shown, so they change nothing
        let mut ambient = sh("w", "watch");
        ambient["ambient"] = json!(true);
        assert_eq!(background_names(&n, &level(vec![json!({ "task_id": "g", "task_type": "local_agent", "description": "Review the diff" }), ambient])), n);
        assert_eq!(background_names(&n, &json!({ "type": "assistant" })), n);
    }

    #[test]
    fn port_scan_parsers() {
        // lsof -Fpcn: IPv4 and IPv6 of one port count once; a process with no port is dropped
        let listen = "p3398\ncollama\nf3\nn127.0.0.1:11434\np22884\ncSpotify\nf91\nn*:57621\nf78\nn127.0.0.1:7768\nf92\nn[::1]:7768\np9\ncidle\n";
        assert_eq!(parse_listen(listen), json!([{ "pid": 3398, "name": "ollama", "ports": [11434] }, { "pid": 22884, "name": "Spotify", "ports": [7768, 57621] }]));
        assert_eq!(
            parse_cwd("p3398\nfcwd\nn/opt/homebrew/var\np7458\nfcwd\nn/\n"),
            HashMap::from([(3398, "/opt/homebrew/var".to_string()), (7458, "/".to_string())])
        );
        let ps = parse_ps("  3398  1-02:03:04 /opt/homebrew/bin/ollama serve\n 29492      05:06 python -m http.server 8085\n");
        assert_eq!(ps[&3398], json!({ "secs": 93784, "command": "/opt/homebrew/bin/ollama serve" }));
        assert_eq!(ps[&29492], json!({ "secs": 306, "command": "python -m http.server 8085" }));
        // real docker ps lines (2026-10-22): an unpublished port (8443/tcp) is not on the host
        let dk = "ai1096-copilot-1|0.0.0.0:8000->8000/tcp, [::]:8000->8000/tcp|/Users/o/code/worktrees/AI-1096|17 hours ago|copilot\n".to_string()
            + "ai1096-wiremock-1|8443/tcp, 127.0.0.1:18080->8080/tcp|/Users/o/code/worktrees/AI-1096|17 hours ago|wiremock\n";
        let got: Vec<Value> = parse_docker(&dk).as_array().unwrap().iter().map(|c| json!([c["name"], c["ports"], c["folder"]])).collect();
        assert_eq!(
            got,
            [json!(["ai1096-copilot-1", [8000], "/Users/o/code/worktrees/AI-1096"]), json!(["ai1096-wiremock-1", [18080], "/Users/o/code/worktrees/AI-1096"])]
        );
    }

    #[test]
    fn owner_of_the_deepest_worktree_else_the_topic_folder() {
        let todos = json!([{ "id": 1, "worktree_path": "/w/a" }, { "id": 2, "worktree_path": "/w/a/nested" }, { "id": 3, "worktree_path": null }]);
        let projects = json!([{ "id": 7, "local_path": "/code/app" }, { "id": 8, "local_path": null }]);
        assert_eq!(owner_of("/w/a/nested/src", &todos, &projects), json!({ "todo_id": 2, "project_id": null }));
        assert_eq!(owner_of("/w/a", &todos, &projects), json!({ "todo_id": 1, "project_id": null }));
        assert_eq!(owner_of("/w/ab", &todos, &projects), json!({ "todo_id": null, "project_id": null })); // a sibling folder is not inside
        assert_eq!(owner_of("/code/app/web", &todos, &projects), json!({ "todo_id": null, "project_id": 7 }));
        assert_eq!(owner_of("", &todos, &projects), json!({ "todo_id": null, "project_id": null }));
    }

    #[test]
    fn parse_worktree_list_and_worktree_verdict() {
        let list = parse_worktree_list(
            &[
                "worktree /code/app", "HEAD aaa", "branch refs/heads/main", "",
                "worktree /code/worktrees/fix-x", "HEAD bbb", "branch refs/heads/fix/x", "",
                "worktree /code/worktrees/gone", "HEAD ccc", "detached", "prunable gitdir file points to non-existent location", "",
            ]
            .join("\n"),
        );
        let got: Vec<Value> = list.as_array().unwrap().iter().map(|w| json!([w["path"], w["branch"], w["prunable"]])).collect();
        assert_eq!(got, [json!(["/code/app", "main", false]), json!(["/code/worktrees/fix-x", "fix/x", false]), json!(["/code/worktrees/gone", null, true])]);

        let base = json!({ "prunable": false, "locked": false, "branch": "b", "changed": [], "localOnly": [], "pr": null, "todos": [] });
        let with = |pairs: &[(&str, Value)]| {
            let mut f = base.clone();
            for (k, v) in pairs {
                f[*k] = v.clone();
            }
            f
        };
        let merged = json!({ "number": 591, "url": "u", "state": "merged" });
        // T-108's worktree: PR merged, nothing only here
        assert_eq!(
            worktree_verdict(&with(&[("pr", merged.clone()), ("todos", json!([{ "id": 108, "removed": true }]))])),
            json!({ "verdict": "remove", "reasons": ["PR #591 merged", "T-108 removed"] })
        );
        // renovate-python-packages: PR merged, but 2 commits made after it exist only here
        assert_eq!(
            worktree_verdict(&with(&[("pr", merged.clone()), ("changed", json!(["?? x/"])), ("localOnly", json!(["7dcabeb7 fix", "fe6ccb86 test"]))])),
            json!({ "verdict": "check", "reasons": ["PR #591 merged", "no TODO", "1 changed file", "2 commits on no remote"] })
        );
        // AI-1025: live TODO, no PR, 12 commits only here: kept, with the warning
        assert_eq!(
            worktree_verdict(&with(&[("localOnly", json!(vec!["x"; 12])), ("todos", json!([{ "id": 12, "removed": false }]))])),
            json!({ "verdict": "keep", "reasons": ["no PR", "T-12 live", "12 commits on no remote"] })
        );
        let mut open = merged.clone();
        open["state"] = json!("open");
        assert_eq!(worktree_verdict(&with(&[("pr", open)]))["verdict"], "keep");
        assert_eq!(worktree_verdict(&with(&[("todos", json!([{ "id": 13, "removed": true }, { "id": 19, "removed": false }]))]))["verdict"], "keep");
        assert_eq!(worktree_verdict(&with(&[("pr", merged.clone()), ("locked", json!(true))]))["verdict"], "check");
        assert_eq!(worktree_verdict(&with(&[("branch", Value::Null)]))["reasons"][0], "detached HEAD");
        assert_eq!(worktree_verdict(&with(&[("prunable", json!(true))]))["verdict"], "remove");
        // a branch without worktree: no TODO line
        assert_eq!(
            worktree_verdict(&with(&[("branchOnly", json!(true)), ("pr", merged)])),
            json!({ "verdict": "remove", "reasons": ["PR #591 merged"] })
        );
    }

    #[test]
    fn swap_place_each_topic_keeps_its_worktree_and_pr() {
        let a = json!({ "worktree_path": "/wt/a", "branch": "feat-a", "pr_number": 12 });
        let none = json!({ "worktree_path": null, "branch": null, "pr_number": null });
        let yes = |_: &str| true;
        // A to B: A's place is stored, B has none yet.
        let s1 = swap_place(&json!({}), Some(1), Some(2), &a, yes);
        assert_eq!(s1, json!({ "places": { "1": a }, "back": { "worktree_path": null, "pr_number": null } }));
        // B (nothing registered) back to A: A's place comes back and leaves the store.
        assert_eq!(swap_place(&s1["places"], Some(2), Some(1), &none, yes), json!({ "places": {}, "back": { "worktree_path": "/wt/a", "pr_number": 12 } }));
        // No topic is a key too; a stored worktree whose folder is gone comes back without it.
        let s2 = swap_place(&json!({}), None, Some(1), &a, yes);
        assert_eq!(s2["places"], json!({ "none": a }));
        assert_eq!(swap_place(&s2["places"], Some(1), None, &none, |_| false)["back"], json!({ "worktree_path": null, "pr_number": 12 }));
    }

    // Beyond lib.test.ts: the JS semantics the port keeps by hand.
    #[test]
    fn js_semantics() {
        assert_eq!(slugify("Résumé ﬁle Ｗeb"), "re-sume-file-web");
        assert_eq!(js_number(" 12 "), 12.0);
        assert!(js_number("inf").is_nan());
        assert_eq!(js_number("0x1f"), 31.0);
        assert_eq!(js_dirname("/a/b/"), "/a");
        assert_eq!(js_dirname("/a"), "/");
        assert_eq!(js_dirname("a"), ".");
        assert_eq!(js_dirname("//a"), "//");
        assert_eq!(js_dirname("a/b"), "a");
        assert_eq!(js_basename("/a/b/"), "b");
        assert!(!is_inside("/d/..x", "/d"));
        let ordered = js_order(json!({ "b": 1, "10": 2, "2": 3 }).as_object().unwrap().clone());
        assert_eq!(ordered.keys().collect::<Vec<_>>(), ["2", "10", "b"]);
        assert_eq!(parse_marks(&["R100", "x"].join("\0"), ""), json!({ "x": "D", "undefined": "A" }));
        assert_eq!(cut_to_two_lines("a\u{feff}"), "a");
        assert_eq!(stringify(&json!({ "claude": { "2": "x", "1": "y" } })), r#"{"claude":{"1":"y","2":"x"}}"#);
    }
}

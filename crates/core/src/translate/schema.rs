//! `docs/proxy-behavior.md` §2.4 — the `pattern` keyword, under a validator
//! narrower than the one the schema was written for.

use serde_json::Value;

/// Subschemas held as the values of a map keyword.
const MAP_KEYWORDS: [&str; 5] = [
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];

/// Keywords whose value is a subschema, or a list of them.
const SCHEMA_KEYWORDS: [&str; 15] = [
    "items",
    "prefixItems",
    "additionalItems",
    "additionalProperties",
    "unevaluatedItems",
    "unevaluatedProperties",
    "propertyNames",
    "contains",
    "not",
    "if",
    "then",
    "else",
    "allOf",
    "anyOf",
    "oneOf",
];

/// Drop every `pattern` the backend's validator would refuse, wherever it sits.
///
/// Refusal is not partial: one unsupported pattern anywhere in one tool's
/// schema rejects the whole request, and the client can neither see the reason
/// nor fix it. A dropped pattern costs the model a hint about one argument.
pub(super) fn drop_unsupported_patterns(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };

    if object
        .get("pattern")
        .and_then(Value::as_str)
        .is_some_and(|pattern| !supported(pattern))
    {
        object.remove("pattern");
    }

    // The keys of `patternProperties` are patterns in their own right, and the
    // validator reads them as such.
    if let Some(Value::Object(map)) = object.get_mut("patternProperties") {
        map.retain(|key, _| supported(key));
    }

    for keyword in MAP_KEYWORDS {
        if let Some(Value::Object(map)) = object.get_mut(keyword) {
            for value in map.values_mut() {
                drop_unsupported_patterns(value);
            }
        }
    }

    for keyword in SCHEMA_KEYWORDS {
        match object.get_mut(keyword) {
            Some(Value::Array(values)) => {
                for value in values {
                    drop_unsupported_patterns(value);
                }
            }
            Some(value) => drop_unsupported_patterns(value),
            None => {}
        }
    }
}

/// Whether the backend's validator accepts this pattern.
///
/// It compiles patterns with a dialect that has no Unicode property escapes,
/// no braced code points, and its own spelling for a named group — all of
/// which the client's schemas may legitimately use. The answer here is
/// deliberately narrow: a construct this does not recognize is refused even
/// where the validator would have taken it, because a false accept fails the
/// turn and a false refuse only loses a hint.
fn supported(pattern: &str) -> bool {
    let mut chars = pattern.chars().peekable();
    let mut depth = 0_i32;
    // Whether what came last is something a quantifier may follow, and
    // whether it was itself a quantifier (which only a lazy `?` may follow).
    // A quantifier with nothing to repeat is an error in every dialect.
    let mut quantifiable = false;
    let mut after_quantifier = false;

    while let Some(c) = chars.next() {
        let repeats = matches!(c, '*' | '+' | '?' | '{');
        if !repeats {
            after_quantifier = false;
        }
        match c {
            '\\' => {
                if !escape(chars.next()) {
                    return false;
                }
                quantifiable = true;
            }
            '[' => {
                quantifiable = true;
                if !class(&mut chars) {
                    return false;
                }
            }
            '(' => {
                quantifiable = false;
                depth += 1;
                if chars.peek() == Some(&'?') {
                    chars.next();
                    match chars.next() {
                        // Non-capturing and both lookaheads.
                        Some(':' | '=' | '!') => {}
                        // Lookbehind, either way. A `(?<name>` group is spelled
                        // differently upstream and is refused here.
                        Some('<') => match chars.next() {
                            Some('=' | '!') => {}
                            _ => return false,
                        },
                        _ => return false,
                    }
                }
            }
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
                quantifiable = true;
            }
            '{' => {
                if !quantifiable || !quantifier(&mut chars) {
                    return false;
                }
                quantifiable = false;
                after_quantifier = true;
            }
            // A stray `]` or `}` is a literal in some dialects and an error in
            // others. Refused, on the rule above.
            ']' | '}' => return false,
            '?' if after_quantifier => after_quantifier = false,
            '*' | '+' | '?' => {
                if !quantifiable {
                    return false;
                }
                quantifiable = false;
                after_quantifier = true;
            }
            '|' | '^' | '$' => quantifiable = false,
            '.' => quantifiable = true,
            c if c.is_control() => return false,
            _ => quantifiable = true,
        }
    }

    depth == 0
}

/// What may follow a backslash. Letters are an allow-list; a digit is a group
/// reference the validator resolves and may not find.
fn escape(c: Option<char>) -> bool {
    match c {
        Some(c) if c.is_ascii_alphanumeric() => {
            matches!(
                c,
                'd' | 'D'
                    | 'w'
                    | 'W'
                    | 's'
                    | 'S'
                    | 'b'
                    | 'B'
                    | 'A'
                    | 'Z'
                    | 'n'
                    | 'r'
                    | 't'
                    | 'f'
                    | 'v'
            )
        }
        Some(c) => c.is_ascii_punctuation(),
        None => false,
    }
}

/// A character class, from just past its `[` to its `]`.
fn class(chars: &mut std::iter::Peekable<std::str::Chars>) -> bool {
    if chars.peek() == Some(&'^') {
        chars.next();
    }

    let mut members = 0_u32;
    let mut previous_was_escape = false;

    loop {
        match chars.next() {
            // An empty class is an error upstream, not an unmatchable class.
            Some(']') => return members > 0,
            Some('\\') => {
                if !escape(chars.next()) {
                    return false;
                }
                previous_was_escape = true;
                members += 1;
            }
            Some('-') => {
                // A range endpoint has to be a single character. `[a-\d]` is
                // an error, and so is `[\d-a]`.
                if members > 0
                    && chars.peek() != Some(&']')
                    && (previous_was_escape || chars.peek() == Some(&'\\'))
                {
                    return false;
                }
                previous_was_escape = false;
            }
            Some(c) if c.is_control() => return false,
            Some(_) => {
                previous_was_escape = false;
                members += 1;
            }
            None => return false,
        }
    }
}

/// A repetition count, from just past its `{` to its `}`.
fn quantifier(chars: &mut std::iter::Peekable<std::str::Chars>) -> bool {
    let mut bounds = vec![String::new()];

    loop {
        match chars.next() {
            Some('}') => break,
            Some(c) if c.is_ascii_digit() => bounds.last_mut().map_or((), |b| b.push(c)),
            Some(',') if bounds.len() == 1 => bounds.push(String::new()),
            _ => return false,
        }
    }

    // A minimum is required, and a maximum below it is an error.
    let Some(Ok(minimum)) = bounds.first().map(|b| b.parse::<u64>()) else {
        return false;
    };
    match bounds.get(1).filter(|maximum| !maximum.is_empty()) {
        Some(maximum) => maximum
            .parse::<u64>()
            .is_ok_and(|maximum| minimum <= maximum),
        None => true,
    }
}

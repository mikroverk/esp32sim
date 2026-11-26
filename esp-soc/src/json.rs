//! JSON at the host protocol boundary, shared by Cooja NDJSON and web controls.

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self { Self::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v), _ => None }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self { Self::Int(i) => Some(*i), Self::Float(f) => Some(*f as i64), _ => None }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self { Self::Int(i) => Some(*i as f64), Self::Float(f) => Some(*f), _ => None }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self { Self::Str(s) => Some(s), _ => None }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self { Self::Arr(a) => Some(a), _ => None }
    }
    pub fn i64_or(&self, key: &str, default: i64) -> i64 {
        self.get(key).and_then(Self::as_i64).unwrap_or(default)
    }
    pub fn str_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).and_then(Self::as_str).unwrap_or(default)
    }
    /// Text controls accept either JSON strings or scalar numbers and booleans.
    pub fn scalar_text(&self) -> Option<String> {
        match self {
            Self::Str(s) => Some(s.clone()),
            Self::Int(i) => Some(i.to_string()),
            Self::Float(f) => Some(f.to_string()),
            Self::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
}

const MAX_DEPTH: usize = 64;

struct Parser<'a> { s: &'a [u8], i: usize }

impl Parser<'_> {
    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) { self.i += 1; }
    }
    fn peek(&self) -> Option<u8> { self.s.get(self.i).copied() }
    fn take(&mut self, c: u8) -> bool {
        if self.peek() != Some(c) { return false; }
        self.i += 1;
        true
    }
    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.take(c) { Ok(()) } else { Err(format!("expected '{}' at {}", c as char, self.i)) }
    }
    fn value(&mut self, depth: usize) -> Result<Json, String> {
        self.ws();
        if depth >= MAX_DEPTH && matches!(self.peek(), Some(b'{' | b'[')) {
            return Err("JSON nesting limit exceeded".into());
        }
        match self.peek() {
            Some(b'{') => {
                self.i += 1;
                let mut fields = Vec::new();
                self.ws();
                if self.take(b'}') { return Ok(Json::Obj(fields)); }
                loop {
                    let key = self.string()?;
                    self.ws();
                    self.expect(b':')?;
                    fields.push((key, self.value(depth + 1)?));
                    self.ws();
                    if self.take(b'}') { break; }
                    self.expect(b',')?;
                    self.ws();
                }
                Ok(Json::Obj(fields))
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.take(b']') { return Ok(Json::Arr(items)); }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    if self.take(b']') { break; }
                    self.expect(b',')?;
                }
                Ok(Json::Arr(items))
            }
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.literal(b"true", Json::Bool(true)),
            Some(b'f') => self.literal(b"false", Json::Bool(false)),
            Some(b'n') => self.literal(b"null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(format!("expected JSON value at {}", self.i)),
        }
    }
    fn literal(&mut self, word: &[u8], value: Json) -> Result<Json, String> {
        if !self.s[self.i..].starts_with(word) { return Err(format!("bad literal at {}", self.i)); }
        self.i += word.len();
        Ok(value)
    }
    fn digits(&mut self) -> Result<(), String> {
        let start = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9')) { self.i += 1; }
        if start == self.i { Err(format!("expected digit at {}", self.i)) } else { Ok(()) }
    }
    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        self.take(b'-');
        if !self.take(b'0') { self.digits()?; }
        if self.take(b'.') { self.digits()?; }
        if self.take(b'e') || self.take(b'E') {
            if !self.take(b'+') { self.take(b'-'); }
            self.digits()?;
        }
        // Number syntax above only consumes ASCII, so both ends are UTF-8 boundaries.
        let text = std::str::from_utf8(&self.s[start..self.i]).unwrap();
        if let Ok(value) = text.parse::<i64>() { return Ok(Json::Int(value)); }
        match text.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(Json::Float(value)),
            _ => Err(format!("number out of range at {start}")),
        }
    }
    fn hex_quad(&mut self) -> Result<u32, String> {
        let mut value = 0;
        for _ in 0..4 {
            let digit = match self.peek() {
                Some(c @ b'0'..=b'9') => c - b'0',
                Some(c @ b'a'..=b'f') => c - b'a' + 10,
                Some(c @ b'A'..=b'F') => c - b'A' + 10,
                _ => return Err(format!("bad Unicode escape at {}", self.i)),
            };
            value = value * 16 + u32::from(digit);
            self.i += 1;
        }
        Ok(value)
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        // The plain run up to the first quote, escape or control character is copied at once, so
        // a long string (hex payloads) is one exact allocation rather than a doubling buffer.
        let start = self.i;
        while self.peek().is_some_and(|c| c != b'"' && c != b'\\' && c >= 32) { self.i += 1; }
        let mut out = self.s[start..self.i].to_vec();
        loop {
            let Some(c) = self.peek() else { return Err("unterminated string".into()); };
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).map_err(|e| e.to_string()),
                b'\\' => {
                    let Some(escape) = self.peek() else { return Err("unterminated escape".into()); };
                    self.i += 1;
                    match escape {
                        b'"' | b'\\' | b'/' => out.push(escape),
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let mut cp = self.hex_quad()?;
                            if (0xd800..=0xdbff).contains(&cp) {
                                self.expect(b'\\')?;
                                self.expect(b'u')?;
                                let low = self.hex_quad()?;
                                if !(0xdc00..=0xdfff).contains(&low) { return Err("bad Unicode surrogate pair".into()); }
                                cp = 0x10000 + ((cp - 0xd800) << 10) + low - 0xdc00;
                            }
                            let ch = char::from_u32(cp).ok_or("unpaired Unicode surrogate")?;
                            out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        _ => return Err(format!("bad escape at {}", self.i - 1)),
                    }
                }
                0..=31 => return Err(format!("unescaped control character at {}", self.i - 1)),
                _ => out.push(c),
            }
        }
    }
}

/// Parse one complete JSON value with at most 64 levels of nesting.
pub fn parse_json(text: &str) -> Result<Json, String> {
    let mut parser = Parser { s: text.as_bytes(), i: 0 };
    let value = parser.value(0)?;
    parser.ws();
    if parser.i != parser.s.len() { return Err(format!("trailing data at {}", parser.i)); }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_lookup_obeys_structure_and_decodes_escapes() {
        let value = parse_json(r#"{"nested":{"t":"wrong"},"text":"\"t\":\"wrong\"","\u0074":"key","data":"a\"b\\c\n\r\t\b\f/é\uD83D\uDE80","pin":12,"down":true}"#).unwrap();
        assert_eq!(value.str_or("t", ""), "key");
        assert_eq!(value.str_or("data", ""), "a\"b\\c\n\r\t\u{8}\u{c}/é🚀");
        assert_eq!(value.get("pin").unwrap().scalar_text().as_deref(), Some("12"));
        assert_eq!(value.get("down").unwrap().scalar_text().as_deref(), Some("true"));
        assert_eq!(value.get("nested").unwrap().scalar_text(), None);
        assert_eq!(value.get("missing"), None);
    }

    #[test]
    fn rejects_malformed_json_instead_of_extracting_partial_fields() {
        for text in [
            "", "{\"a\":1} x", "[1,2,]", "{\"a\":1,}", "{a:1}", "[1 2]",
            "01", "-01", "+1", ".1", "1.", "1e", "1e+", "--1", "NaN", "1e9999",
            "\"\\q\"", "\"\\u12\"", "\"\\uXY00\"", "\"\\uD800\"", "\"\\uDC00\"",
            "\"\\uD800\\u0041\"", "\"line\nbreak\"", "\"\u{0}\"", "truefalse",
        ] {
            assert!(parse_json(text).is_err(), "accepted {text:?}");
        }
    }

    #[test]
    fn preserves_protocol_numbers_arrays_and_defaults() {
        let value = parse_json("{\"t\":1500000000,\"x\":-1.5e2,\"in\":[null,false,0,-0,1E+2]}").unwrap();
        assert_eq!(value.i64_or("t", 0), 1_500_000_000);
        assert_eq!(value.get("x").unwrap().as_f64(), Some(-150.0));
        assert_eq!(value.i64_or("missing", 17), 17);
        assert_eq!(value.get("in").unwrap().as_arr().unwrap().len(), 5);
        assert_eq!(parse_json("9223372036854775807").unwrap().as_i64(), Some(i64::MAX));
        assert_eq!(parse_json("-9223372036854775808").unwrap().as_i64(), Some(i64::MIN));
        assert_eq!(parse_json("1.9").unwrap().as_i64(), Some(1));
        assert_eq!(parse_json("{\"t\":1,\"t\":2}").unwrap().i64_or("t", 0), 1);
    }

    #[test]
    fn rejects_excessive_nesting_before_stack_exhaustion() {
        let nested = |depth| format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        assert!(parse_json(&nested(MAX_DEPTH)).is_ok());
        assert!(parse_json(&nested(MAX_DEPTH + 1)).is_err());
        assert!(parse_json(&nested(10_000)).is_err());
        assert!(parse_json(&format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1))).is_err());
    }
}

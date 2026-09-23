//! Minimal FCS 3.0/3.1 ASCII event-data reader/writer.
//!
//! Scope is deliberately narrow: the offline case pack uses ASCII mode
//! (`$DATATYPE/ASCII`, `$MODE/L`) so that non-numeric sentinel tokens such as
//! "OVER" can be kept verbatim. Numeric values are widened to f64 by callers
//! for computation, but the original token text is always preserved.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub n: String,
    pub detector: Option<String>,
    pub range: f64,
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FcsValue {
    /// Original textual representation from the file (lossless).
    pub token: String,
    /// Numeric interpretation, if the token parses as a float.
    pub numeric: Option<f64>,
    /// Diagnostic attached when `numeric` is None or non-finite.
    pub diagnostic: Option<String>,
}

impl FcsValue {
    pub fn from_token(token: &str) -> Self {
        let numeric = token.trim().parse::<f64>().ok().filter(|v| v.is_finite());
        let diagnostic = if numeric.is_none() {
            Some(format!("token \"{}\" is not a finite float; retained verbatim", token.trim()))
        } else {
            None
        };
        FcsValue {
            token: token.trim().to_string(),
            numeric,
            diagnostic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FcsFile {
    pub version: String,
    pub keywords: Vec<(String, String)>,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<Vec<FcsValue>>,
}

impl FcsFile {
    pub fn keyword(&self, key: &str) -> Option<&str> {
        self.keywords
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Write a deterministic FCS 3.1 file: HEADER(256) TEXT DATA.
    pub fn to_bytes(&self) -> Vec<u8> {
        let delim = '/';
        let mut text = String::new();
        text.push(delim);
        let mut push = |k: &str, v: &str, text: &mut String| {
            text.push_str(k);
            text.push(delim);
            // escape any delimiter in the value by doubling it
            for ch in v.chars() {
                if ch == delim {
                    text.push(delim);
                }
                text.push(ch);
            }
            text.push(delim);
        };
        let par = self.channels.len();
        let tot = self.events.len();
        let fixed = [
            ("$BEGINANALYSIS", "0"),
            ("$ENDANALYSIS", "0"),
            ("$BEGINSTEXT", "0"),
            ("$ENDSTEXT", "0"),
            ("$NEXTDATA", "0"),
            ("$TOT", &tot.to_string()),
            ("$PAR", &par.to_string()),
            ("$MODE", "L"),
            ("$BYTEORD", "1,2,3,4"),
            ("$DATATYPE", "ASCII"),
        ];
        for (k, v) in fixed {
            push(k, v, &mut text);
        }
        for (i, ch) in self.channels.iter().enumerate() {
            let p = i + 1;
            push(&format!("$P{p}N"), &ch.n, &mut text);
            if let Some(d) = &ch.detector {
                push(&format!("$P{p}S"), d, &mut text);
            }
            push(&format!("$P{p}R"), &format!("{}", ch.range as u64), &mut text);
            push(&format!("$P{p}E"), "0,0", &mut text);
            push(&format!("$P{p}B"), "32", &mut text);
            if let Some(u) = &ch.unit {
                push(&format!("$P{p}U"), u, &mut text);
            }
        }
        for (k, v) in &self.keywords {
            push(k, v, &mut text);
        }

        let mut data = String::new();
        for (i, ev) in self.events.iter().enumerate() {
            for (j, val) in ev.iter().enumerate() {
                if j > 0 {
                    data.push(',');
                }
                data.push_str(&val.token);
            }
            if i + 1 < self.events.len() {
                data.push('\n');
            }
        }

        let text_begin: u64 = 256;
        let text_end = text_begin + text.len() as u64 - 1;
        let data_begin = text_end + 1;
        let data_end = data_begin + data.len() as u64 - 1;
        let field = |v: u64| format!("{:>8}", v);
        let mut header = format!(
            "{:<8}{}{}{}{}{}{}{}flowline offline case pack",
            self.version,
            field(text_begin),
            field(text_end),
            field(data_begin),
            field(data_end),
            field(0),
            field(0),
            field(0),
        );
        if header.len() > 256 {
            header.truncate(256);
        } else {
            while header.len() < 256 {
                header.push(' ');
            }
        }
        let mut out = header.into_bytes();
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(data.as_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<FcsFile, String> {
        if bytes.len() < 58 {
            return Err("file shorter than FCS header".to_string());
        }
        let version = String::from_utf8_lossy(&bytes[0..8]).trim().to_string();
        let read_off = |start: usize| -> u64 {
            String::from_utf8_lossy(&bytes[start..start + 8])
                .trim()
                .parse()
                .unwrap_or(0)
        };
        let text_begin = read_off(10) as usize;
        let text_end = read_off(18) as usize;
        let data_begin = read_off(26) as usize;
        let data_end = read_off(34) as usize;
        if text_begin == 0 || text_end < text_begin || data_end < data_begin {
            return Err("invalid FCS segment offsets".to_string());
        }
        let text = String::from_utf8_lossy(&bytes[text_begin..=text_end]).to_string();
        let delim = text.chars().next().ok_or("empty TEXT segment")?;
        // TEXT begins and ends with the delimiter; split, honoring doubled delims.
        let mut kvs: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut chars = text.chars().peekable();
        chars.next(); // opening delimiter
        while let Some(ch) = chars.next() {
            if ch == delim {
                if chars.peek() == Some(&delim) {
                    cur.push(delim);
                    chars.next();
                } else {
                    kvs.push(std::mem::take(&mut cur));
                }
            } else {
                cur.push(ch);
            }
        }
        if kvs.len() % 2 != 0 {
            return Err("TEXT segment has odd number of keyword tokens".to_string());
        }
        let mut keywords: Vec<(String, String)> = Vec::new();
        let mut get = |key: &str| -> Option<String> {
            kvs.chunks(2)
                .find(|kv| kv[0].eq_ignore_ascii_case(key))
                .map(|kv| kv[1].clone())
        };
        for chunk in kvs.chunks(2) {
            keywords.push((chunk[0].clone(), chunk[1].clone()));
        }
        let par: usize = get("$PAR")
            .ok_or("missing $PAR")?
            .parse()
            .map_err(|_| "bad $PAR")?;
        let tot: usize = get("$TOT")
            .ok_or("missing $TOT")?
            .parse()
            .map_err(|_| "bad $TOT")?;
        let datatype = get("$DATATYPE").unwrap_or_default();
        if !datatype.eq_ignore_ascii_case("ASCII") {
            return Err(format!("only ASCII FCS data supported, got {datatype}"));
        }
        let mut channels = Vec::with_capacity(par);
        for i in 1..=par {
            let n = get(&format!("$P{i}N")).ok_or_else(|| format!("missing $P{i}N"))?;
            let detector = get(&format!("$P{i}S"));
            let range = get(&format!("$P{i}R"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            let unit = get(&format!("$P{i}U"));
            channels.push(ChannelSpec {
                n,
                detector,
                range,
                unit,
            });
        }
        let data = String::from_utf8_lossy(&bytes[data_begin..=data_end]);
        let tokens: Vec<&str> = data
            .split(|c: char| c == ',' || c == delim || c.is_whitespace())
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.len() != par * tot {
            return Err(format!(
                "expected {} values ({} par x {} events), found {}",
                par * tot,
                par,
                tot,
                tokens.len()
            ));
        }
        let mut events = Vec::with_capacity(tot);
        for e in 0..tot {
            let row: Vec<FcsValue> = tokens[e * par..(e + 1) * par]
                .iter()
                .map(|t| FcsValue::from_token(t))
                .collect();
            events.push(row);
        }
        Ok(FcsFile {
            version,
            keywords,
            channels,
            events,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> FcsFile {
        FcsFile {
            version: "FCS3.1".to_string(),
            keywords: vec![("$FIL".to_string(), "sample.fcs".to_string())],
            channels: vec![
                ChannelSpec {
                    n: "FSC-A".to_string(),
                    detector: Some("FSC".to_string()),
                    range: 262_144.0,
                    unit: Some("ADC counts".to_string()),
                },
                ChannelSpec {
                    n: "FITC-A".to_string(),
                    detector: Some("FITC".to_string()),
                    range: 262_144.0,
                    unit: Some("ADC counts".to_string()),
                },
            ],
            events: vec![
                vec![
                    FcsValue::from_token("100"),
                    FcsValue::from_token("200.5"),
                ],
                vec![
                    FcsValue::from_token("-5"),
                    FcsValue::from_token("OVER"),
                ],
            ],
        }
    }

    #[test]
    fn roundtrip_preserves_tokens_and_diagnostics() {
        let f = sample();
        let bytes = f.to_bytes();
        let g = FcsFile::from_bytes(&bytes).unwrap();
        assert_eq!(f, g);
        assert_eq!(g.events[1][1].numeric, None);
        assert!(g.events[1][1].diagnostic.is_some());
        assert_eq!(g.keyword("$FIL"), Some("sample.fcs"));
    }
}

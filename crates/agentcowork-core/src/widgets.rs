//! P8.2 Widget Cards (H17 — doc 35 §B Vane pattern).
//!
//! Inline chat widgets, rendered as structured [`WidgetCard`]s the UI turns
//! into cards. All logic is pure and fully testable:
//!
//! - [`MathWidget::evaluate`] — a safe calculator (recursive-descent parser,
//!   no `eval`, no unsafe): `+ - * / % ^` and parentheses.
//! - [`LookupWidget`] — a generic key/value lookup card.
//! - [`WeatherWidget`] / [`StockWidget`] — cards over injected snapshots with
//!   a TTL cache. Live fetching (a weather/stock API) is a documented runtime
//!   seam; the card + cache logic is here and tested with injected data.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A structured widget card the UI renders. `kind` drives the card template.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WidgetCard {
    pub kind: String,
    pub title: String,
    /// Ordered rows: `(label, value, unit?)`.
    pub rows: Vec<(String, String, Option<String>)>,
    pub footer: Option<String>,
}

impl WidgetCard {
    pub fn simple(
        kind: &str,
        title: impl Into<String>,
        rows: Vec<(String, String, Option<String>)>,
    ) -> Self {
        Self {
            kind: kind.to_string(),
            title: title.into(),
            rows,
            footer: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetError {
    /// A math expression could not be parsed or evaluated.
    Math(String),
    /// A lookup/weather/stock card was requested with no data.
    NoData,
}

impl std::fmt::Display for WidgetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WidgetError::Math(m) => write!(f, "math: {m}"),
            WidgetError::NoData => write!(f, "widget: no data"),
        }
    }
}

impl std::error::Error for WidgetError {}

// ---------------------------------------------------------------------------
// Math / calculator widget
// ---------------------------------------------------------------------------

/// A safe calculator. Grammar (recursive descent):
/// `expr := term (('+'|'-') term)*`
/// `term := factor (('*'|'/'|'%') factor)*`
/// `factor := unary ('^' factor)?`
/// `unary := ('-'|'+')? primary`
/// `primary := number | '(' expr ')'`
pub struct MathWidget;

impl MathWidget {
    pub fn evaluate(input: &str) -> Result<f64, WidgetError> {
        let tokens = tokenize_math(input)?;
        if tokens.is_empty() {
            return Err(WidgetError::Math("empty expression".into()));
        }
        let mut p = MathParser {
            tokens: &tokens,
            pos: 0,
        };
        let value = p.parse_expr().map_err(WidgetError::Math)?;
        if p.pos != p.tokens.len() {
            return Err(WidgetError::Math(format!(
                "unexpected trailing token `{:?}`",
                p.tokens[p.pos]
            )));
        }
        Ok(value)
    }

    /// Render the answer as a card.
    pub fn card(input: &str) -> Result<WidgetCard, WidgetError> {
        let value = Self::evaluate(input)?;
        Ok(WidgetCard::simple(
            "math",
            "Calculator",
            vec![("expression".into(), input.trim().into(), None)],
        )
        .with_answer(value))
    }
}

impl WidgetCard {
    fn with_answer(mut self, value: f64) -> Self {
        let (v, unit) = format_number(value);
        self.rows.push(("result".into(), v, unit));
        self
    }
}

fn format_number(v: f64) -> (String, Option<String>) {
    if v.is_finite() && v.fract().abs() < 1e-9 && v.abs() < 1e15 {
        (format!("{}", v as i64), None)
    } else {
        (format!("{v:.6}"), None)
    }
}

/// Token types for the math parser.
#[derive(Debug, Clone, PartialEq)]
enum MTok {
    Num(f64),
    Op(char),
    LParen,
    RParen,
}

fn tokenize_math(s: &str) -> Result<Vec<MTok>, WidgetError> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let num: String = chars[start..i].iter().collect();
            let v: f64 = num
                .parse()
                .map_err(|_| WidgetError::Math(format!("bad number `{num}`")))?;
            out.push(MTok::Num(v));
            continue;
        }
        match c {
            '+' | '-' | '*' | '/' | '%' | '^' => out.push(MTok::Op(c)),
            '(' => out.push(MTok::LParen),
            ')' => out.push(MTok::RParen),
            other => return Err(WidgetError::Math(format!("unexpected character `{other}`"))),
        }
        i += 1;
    }
    Ok(out)
}

struct MathParser<'a> {
    tokens: &'a [MTok],
    pos: usize,
}

impl<'a> MathParser<'a> {
    fn peek(&self) -> Option<&MTok> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<MTok> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_expr(&mut self) -> Result<f64, String> {
        let mut left = self.parse_term()?;
        while let Some(MTok::Op('+')) | Some(MTok::Op('-')) = self.peek() {
            let op = match self.next() {
                Some(MTok::Op(o)) => o,
                _ => unreachable!(),
            };
            let right = self.parse_term()?;
            left = if op == '+' {
                left + right
            } else {
                left - right
            };
        }
        Ok(left)
    }

    fn parse_term(&mut self) -> Result<f64, String> {
        let mut left = self.parse_factor()?;
        while let Some(MTok::Op('*')) | Some(MTok::Op('/')) | Some(MTok::Op('%')) = self.peek() {
            let op = match self.next() {
                Some(MTok::Op(o)) => o,
                _ => unreachable!(),
            };
            let right = self.parse_factor()?;
            left = match op {
                '*' => left * right,
                '/' => {
                    if right == 0.0 {
                        return Err("division by zero".into());
                    }
                    left / right
                }
                '%' => {
                    if right == 0.0 {
                        return Err("modulo by zero".into());
                    }
                    left % right
                }
                _ => unreachable!(),
            };
        }
        Ok(left)
    }

    fn parse_factor(&mut self) -> Result<f64, String> {
        let base = self.parse_unary()?;
        if let Some(MTok::Op('^')) = self.peek() {
            self.next();
            let exp = self.parse_factor()?; // right-associative
            return Ok(base.powf(exp));
        }
        Ok(base)
    }

    fn parse_unary(&mut self) -> Result<f64, String> {
        if let Some(MTok::Op('-')) = self.peek() {
            self.next();
            return Ok(-self.parse_unary()?);
        }
        if let Some(MTok::Op('+')) = self.peek() {
            self.next();
            return self.parse_unary();
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<f64, String> {
        match self.next() {
            Some(MTok::Num(v)) => Ok(v),
            Some(MTok::LParen) => {
                let v = self.parse_expr()?;
                match self.next() {
                    Some(MTok::RParen) => Ok(v),
                    _ => Err("expected `)`".into()),
                }
            }
            Some(t) => Err(format!("unexpected token {t:?}")),
            None => Err("unexpected end of expression".into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Generic lookup widget
// ---------------------------------------------------------------------------

/// A generic key/value lookup card (e.g. a dictionary entry, a unit
/// conversion, a config lookup).
pub struct LookupWidget;

impl LookupWidget {
    pub fn card(title: &str, pairs: &[(String, String)], footer: Option<&str>) -> WidgetCard {
        let mut card = WidgetCard::simple(
            "lookup",
            title,
            pairs
                .iter()
                .map(|(k, v)| (k.clone(), v.clone(), None))
                .collect(),
        );
        card.footer = footer.map(|s| s.to_string());
        card
    }
}

// ---------------------------------------------------------------------------
// Weather widget (injected snapshot + TTL cache; live API is a seam)
// ---------------------------------------------------------------------------

/// A weather snapshot produced by an external source (injected for tests).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WeatherSnapshot {
    pub location: String,
    pub condition: String,
    pub temp_c: f64,
    pub feels_like_c: f64,
    pub humidity_pct: u8,
    pub wind_kph: f64,
    pub updated_at_ms: u64,
}

pub struct WeatherWidget {
    cache: std::sync::Mutex<HashMap<String, (WeatherSnapshot, Instant)>>,
    ttl: Duration,
}

impl Default for WeatherWidget {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}

impl WeatherWidget {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: std::sync::Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Cache-check + store. Returns `true` when a fresh value was inserted
    /// (the caller should fetch from the live API) and `false` when a cached
    /// value is still fresh (the caller should render the cache).
    pub fn should_refresh(&self, location: &str) -> bool {
        let mut cache = self.cache.lock().unwrap();
        match cache.get(location) {
            Some((_, at)) if at.elapsed() < self.ttl => false,
            _ => {
                // Insert a placeholder timestamp so concurrent calls agree.
                cache.insert(
                    location.to_string(),
                    (
                        WeatherSnapshot {
                            location: location.to_string(),
                            condition: String::new(),
                            temp_c: 0.0,
                            feels_like_c: 0.0,
                            humidity_pct: 0,
                            wind_kph: 0.0,
                            updated_at_ms: 0,
                        },
                        Instant::now(),
                    ),
                );
                true
            }
        }
    }

    /// Store a fresh snapshot (call after a live fetch).
    pub fn store(&self, snapshot: WeatherSnapshot) {
        let mut cache = self.cache.lock().unwrap();
        cache.insert(snapshot.location.clone(), (snapshot, Instant::now()));
    }

    pub fn get(&self, location: &str) -> Option<WeatherSnapshot> {
        let cache = self.cache.lock().unwrap();
        cache.get(location).map(|(s, _)| s.clone())
    }

    /// Render a weather card from an injected snapshot.
    pub fn card(snapshot: &WeatherSnapshot) -> WidgetCard {
        let mut card = WidgetCard::simple(
            "weather",
            format!("Weather · {}", snapshot.location),
            vec![
                ("condition".into(), snapshot.condition.clone(), None),
                (
                    "temperature".into(),
                    format!("{:.1}", snapshot.temp_c),
                    Some("°C".into()),
                ),
                (
                    "feels like".into(),
                    format!("{:.1}", snapshot.feels_like_c),
                    Some("°C".into()),
                ),
                (
                    "humidity".into(),
                    format!("{}", snapshot.humidity_pct),
                    Some("%".into()),
                ),
                (
                    "wind".into(),
                    format!("{:.1}", snapshot.wind_kph),
                    Some("km/h".into()),
                ),
            ],
        );
        card.footer = Some(format!("updated {}", snapshot.updated_at_ms));
        card
    }
}

// ---------------------------------------------------------------------------
// Stock / finance widget (injected snapshot + TTL cache; live API is a seam)
// ---------------------------------------------------------------------------

/// A stock quote produced by an external source (injected for tests).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StockQuote {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub change_pct: f64,
    pub currency: String,
    pub updated_at_ms: u64,
}

pub struct StockWidget {
    cache: std::sync::Mutex<HashMap<String, (StockQuote, Instant)>>,
    ttl: Duration,
}

impl Default for StockWidget {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}

impl StockWidget {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: std::sync::Mutex::new(HashMap::new()),
            ttl,
        }
    }

    pub fn should_refresh(&self, symbol: &str) -> bool {
        let mut cache = self.cache.lock().unwrap();
        match cache.get(symbol) {
            Some((_, at)) if at.elapsed() < self.ttl => false,
            _ => {
                cache.insert(
                    symbol.to_string(),
                    (
                        StockQuote {
                            symbol: symbol.to_string(),
                            name: String::new(),
                            price: 0.0,
                            change_pct: 0.0,
                            currency: String::new(),
                            updated_at_ms: 0,
                        },
                        Instant::now(),
                    ),
                );
                true
            }
        }
    }

    pub fn store(&self, quote: StockQuote) {
        let mut cache = self.cache.lock().unwrap();
        cache.insert(quote.symbol.clone(), (quote, Instant::now()));
    }

    pub fn get(&self, symbol: &str) -> Option<StockQuote> {
        let cache = self.cache.lock().unwrap();
        cache.get(symbol).map(|(q, _)| q.clone())
    }

    /// Render a stock card from an injected quote. Green/red is a UI concern;
    /// the card carries the signed change for the UI to color.
    pub fn card(quote: &StockQuote) -> WidgetCard {
        WidgetCard::simple(
            "stock",
            format!("{} · {}", quote.symbol, quote.name),
            vec![
                (
                    "price".into(),
                    format!("{:.2}", quote.price),
                    Some(quote.currency.clone()),
                ),
                ("change".into(), format!("{:+.2}%", quote.change_pct), None),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_evaluates_basic_ops() {
        assert_eq!(MathWidget::evaluate("2+3*4").unwrap(), 14.0);
        assert_eq!(MathWidget::evaluate("(2+3)*4").unwrap(), 20.0);
        assert_eq!(MathWidget::evaluate("10/4").unwrap(), 2.5);
        assert_eq!(MathWidget::evaluate("7%3").unwrap(), 1.0);
        assert_eq!(MathWidget::evaluate("2^10").unwrap(), 1024.0);
        assert_eq!(MathWidget::evaluate("2^-2").unwrap(), 0.25);
        assert_eq!(MathWidget::evaluate("-3+5").unwrap(), 2.0);
        assert_eq!(
            MathWidget::evaluate("2^3^2").unwrap(),
            512.0,
            "right-associative ^"
        );
    }

    #[test]
    fn math_rejects_bad_input() {
        assert!(matches!(
            MathWidget::evaluate("1/0"),
            Err(WidgetError::Math(_))
        ));
        assert!(matches!(
            MathWidget::evaluate("1%0"),
            Err(WidgetError::Math(_))
        ));
        assert!(matches!(
            MathWidget::evaluate("(1+2"),
            Err(WidgetError::Math(_))
        ));
        assert!(matches!(
            MathWidget::evaluate("1+"),
            Err(WidgetError::Math(_))
        ));
        assert!(matches!(
            MathWidget::evaluate("abc"),
            Err(WidgetError::Math(_))
        ));
        assert!(matches!(
            MathWidget::evaluate(""),
            Err(WidgetError::Math(_))
        ));
        assert!(matches!(
            MathWidget::evaluate("1 2"),
            Err(WidgetError::Math(_))
        ));
    }

    #[test]
    fn math_card_has_result() {
        let card = MathWidget::card("2+2").unwrap();
        assert_eq!(card.kind, "math");
        assert_eq!(card.rows.last().unwrap().0, "result");
        assert_eq!(card.rows.last().unwrap().1, "4");
    }

    #[test]
    fn lookup_card_renders_pairs() {
        let card = LookupWidget::card(
            "Unit conversion",
            &[("1 inch".into(), "2.54 cm".into())],
            Some("US customary → metric"),
        );
        assert_eq!(card.kind, "lookup");
        assert_eq!(card.rows.len(), 1);
        assert_eq!(card.footer.as_deref(), Some("US customary → metric"));
    }

    #[test]
    fn weather_card_and_ttl_cache() {
        let w = WeatherWidget::default();
        // First call → should refresh.
        assert!(w.should_refresh("London"));
        // Placeholder present, but a real snapshot is stored after fetch.
        w.store(WeatherSnapshot {
            location: "London".into(),
            condition: "Cloudy".into(),
            temp_c: 17.5,
            feels_like_c: 16.0,
            humidity_pct: 70,
            wind_kph: 12.0,
            updated_at_ms: 1,
        });
        // Within TTL → no refresh.
        assert!(!w.should_refresh("London"));
        let snap = w.get("London").unwrap();
        assert_eq!(snap.condition, "Cloudy");
        let card = WeatherWidget::card(&snap);
        assert_eq!(card.kind, "weather");
        assert!(card.title.contains("London"));
        assert!(
            card.rows
                .iter()
                .any(|(l, v, u)| l == "temperature" && v == "17.5" && u.as_deref() == Some("°C"))
        );
    }

    #[test]
    fn weather_ttl_expires() {
        let w = WeatherWidget::new(Duration::from_millis(10));
        assert!(w.should_refresh("Paris"));
        std::thread::sleep(Duration::from_millis(30));
        assert!(w.should_refresh("Paris"), "expired TTL must refresh");
    }

    #[test]
    fn stock_card_and_cache() {
        let s = StockWidget::default();
        assert!(s.should_refresh("AAPL"));
        s.store(StockQuote {
            symbol: "AAPL".into(),
            name: "Apple Inc.".into(),
            price: 218.5,
            change_pct: 1.25,
            currency: "USD".into(),
            updated_at_ms: 1,
        });
        assert!(!s.should_refresh("AAPL"));
        let q = s.get("AAPL").unwrap();
        assert_eq!(q.name, "Apple Inc.");
        let card = StockWidget::card(&q);
        assert_eq!(card.kind, "stock");
        assert!(card.title.contains("AAPL"));
        assert!(
            card.rows
                .iter()
                .any(|(l, v, u)| l == "change" && v == "+1.25%" && u.is_none())
        );
    }
}

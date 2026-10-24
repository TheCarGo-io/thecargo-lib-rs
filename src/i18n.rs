use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    En,
    Ru,
    Uz,
}

pub const DEFAULT_LANG: Lang = Lang::En;

impl Lang {
    pub fn from_accept_language(raw: Option<&str>) -> Self {
        let first = raw.unwrap_or("en").split(',').next().unwrap_or_default();
        match first.split('-').next().unwrap_or_default().trim().to_ascii_lowercase().as_str() {
            "ru" => Self::Ru,
            "uz" => Self::Uz,
            _ => Self::En,
        }
    }
}

#[derive(Debug, Default)]
pub struct Locales {
    tables: HashMap<Lang, HashMap<String, String>>,
}

impl Locales {
    pub fn from_json(en: &str, ru: &str, uz: &str) -> Result<Self, serde_json::Error> {
        let tables = [(Lang::En, en), (Lang::Ru, ru), (Lang::Uz, uz)]
            .into_iter()
            .map(|(lang, raw)| Ok((lang, serde_json::from_str(raw)?)))
            .collect::<Result<_, serde_json::Error>>()?;
        Ok(Self { tables })
    }

    pub fn translate(&self, key: &str, lang: Lang, params: &Map<String, Value>) -> Option<String> {
        let template = self.lookup(lang, key).or_else(|| self.lookup(DEFAULT_LANG, key))?;
        Some(format_template(template, params).unwrap_or_else(|| template.to_owned()))
    }

    fn lookup(&self, lang: Lang, key: &str) -> Option<&str> {
        self.tables.get(&lang)?.get(key).map(String::as_str)
    }
}

static BOUND: OnceLock<Locales> = OnceLock::new();
static EMPTY: OnceLock<Locales> = OnceLock::new();

pub fn bind_locales(locales: Locales) {
    if BOUND.set(locales).is_err() {
        tracing::debug!("locales were already bound");
    }
}

pub fn translate(key: &str, lang: Lang, params: &Map<String, Value>) -> Option<String> {
    BOUND.get().unwrap_or_else(|| EMPTY.get_or_init(Locales::default)).translate(key, lang, params)
}

// Python's str.format(**params) for plain {name} fields: a missing name leaves the template as it is.
fn format_template(template: &str, params: &Map<String, Value>) -> Option<String> {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let name: String = chars.by_ref().take_while(|c| *c != '}').collect();
                out.push_str(&python_str(params.get(&name)?));
            }
            _ => out.push(ch),
        }
    }
    Some(out)
}

fn python_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn locales() -> Locales {
        Locales::from_json(
            r#"{"greet": "{name} is {age}", "plain": "Plain", "braces": "{{x}} {field}"}"#,
            r#"{"plain": "Простой"}"#,
            "{}",
        )
        .unwrap()
    }

    fn params(value: Value) -> Map<String, Value> {
        let Value::Object(map) = value else { panic!("params must be an object") };
        map
    }

    #[test]
    fn the_language_is_the_first_tag_of_accept_language() {
        assert_eq!(Lang::from_accept_language(Some("ru-RU,ru;q=0.9")), Lang::Ru);
        assert_eq!(Lang::from_accept_language(Some(" UZ ")), Lang::Uz);
        assert_eq!(Lang::from_accept_language(Some("de-DE,ru")), Lang::En);
        assert_eq!(Lang::from_accept_language(None), Lang::En);
    }

    #[test]
    fn a_missing_translation_falls_back_to_english() {
        let locales = locales();
        assert_eq!(locales.translate("plain", Lang::Ru, &Map::new()).as_deref(), Some("Простой"));
        assert_eq!(locales.translate("plain", Lang::Uz, &Map::new()).as_deref(), Some("Plain"));
        assert_eq!(locales.translate("absent", Lang::En, &Map::new()), None);
    }

    #[test]
    fn templates_format_like_python() {
        let locales = locales();
        let filled = locales.translate("greet", Lang::En, &params(json!({"name": "Ann", "age": 7})));
        assert_eq!(filled.as_deref(), Some("Ann is 7"));
        let missing = locales.translate("greet", Lang::En, &params(json!({"name": "Ann"})));
        assert_eq!(missing.as_deref(), Some("{name} is {age}"));
        let escaped = locales.translate("braces", Lang::En, &params(json!({"field": null})));
        assert_eq!(escaped.as_deref(), Some("{x} None"));
    }
}

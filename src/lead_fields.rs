use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value, json};

pub const DATE_FORMATS: [&str; 12] = [
    "%m/%d/%Y",
    "%m-%d-%Y",
    "%Y-%m-%d",
    "%m/%d/%y",
    "%d/%m/%Y",
    "%B %d, %Y",
    "%b %d, %Y",
    "%m.%d.%Y",
    "%Y/%m/%d",
    "%d-%m-%Y",
    "%m %d %Y",
    "%d %b %Y",
];

pub const SECTION_ORDER: [&str; 4] = ["SHIPPER", "LOCATION", "VEHICLE", "SHIPMENT"];
pub const REQUIRED_LEAD_FIELD_KEYS: [&str; 7] =
    ["first_name", "phone", "pickup_city", "delivery_city", "year", "make", "model"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    pub key: &'static str,
    pub section: &'static str,
    pub label: &'static str,
    pub item_name: &'static str,
    pub labels: &'static [&'static str],
}

pub const LEAD_FIELD_CATALOG: [FieldSpec; 18] = [
    FieldSpec {
        key: "first_name",
        section: "SHIPPER",
        label: "FIRST NAME",
        item_name: "customer_first_name",
        labels: &[
            "first name",
            "fname",
            "given name",
            "full name",
            "customer name",
            "shipper name",
            "contact name",
            "name",
        ],
    },
    FieldSpec {
        key: "last_name",
        section: "SHIPPER",
        label: "LAST NAME",
        item_name: "customer_last_name",
        labels: &["last name", "lname", "surname", "family name"],
    },
    FieldSpec {
        key: "phone",
        section: "SHIPPER",
        label: "PHONE",
        item_name: "customer_phone",
        labels: &[
            "phone number",
            "contact number",
            "phone no",
            "telephone",
            "cell phone",
            "mobile",
            "phone",
            "tel",
            "cell",
        ],
    },
    FieldSpec {
        key: "email",
        section: "SHIPPER",
        label: "EMAIL",
        item_name: "customer_email",
        labels: &["email address", "e-mail address", "e-mail", "email"],
    },
    FieldSpec {
        key: "pickup_city",
        section: "LOCATION",
        label: "PICKUP CITY",
        item_name: "origin_city",
        labels: &[
            "moving from city",
            "pickup city",
            "pick up city",
            "origin city",
            "from city",
            "pickup location",
            "origin location",
            "pickup",
            "origin",
        ],
    },
    FieldSpec {
        key: "pickup_state",
        section: "LOCATION",
        label: "PICKUP STATE",
        item_name: "origin_state",
        labels: &["moving from state", "pickup state", "pick up state", "origin state", "from state"],
    },
    FieldSpec {
        key: "pickup_zip",
        section: "LOCATION",
        label: "PICKUP ZIP",
        item_name: "origin_zip",
        labels: &[
            "moving from zip code",
            "moving from zipcode",
            "moving from zip",
            "pickup zip code",
            "pickup zipcode",
            "pickup zip",
            "pick up zip",
            "origin zip code",
            "origin zipcode",
            "origin zip",
            "pickup postal code",
            "origin postal code",
            "from zip code",
            "from zipcode",
            "from zip",
        ],
    },
    FieldSpec {
        key: "delivery_city",
        section: "LOCATION",
        label: "DELIVERY CITY",
        item_name: "destination_city",
        labels: &[
            "moving to city",
            "delivery city",
            "destination city",
            "dropoff city",
            "drop off city",
            "to city",
            "dest city",
            "moving city",
            "delivery location",
            "destination location",
            "dropoff location",
            "drop off location",
            "delivery",
            "destination",
        ],
    },
    FieldSpec {
        key: "delivery_state",
        section: "LOCATION",
        label: "DELIVERY STATE",
        item_name: "destination_state",
        labels: &[
            "moving to state",
            "delivery state",
            "destination state",
            "dropoff state",
            "drop off state",
            "dest state",
            "moving state",
            "to state",
        ],
    },
    FieldSpec {
        key: "delivery_zip",
        section: "LOCATION",
        label: "DELIVERY ZIP",
        item_name: "destination_zip",
        labels: &[
            "moving to zip code",
            "moving to zipcode",
            "moving to zip",
            "delivery zip code",
            "delivery zipcode",
            "delivery zip",
            "destination zip code",
            "destination zipcode",
            "destination zip",
            "dropoff zip",
            "drop off zip",
            "dest zip",
            "moving zip",
            "delivery postal code",
            "destination postal code",
            "to zip code",
            "to zipcode",
            "to zip",
        ],
    },
    FieldSpec {
        key: "year",
        section: "VEHICLE",
        label: "YEAR",
        item_name: "vehicle_year",
        labels: &["year of the vehicle", "year of vehicle", "vehicle year", "car year", "auto year", "year"],
    },
    FieldSpec {
        key: "make",
        section: "VEHICLE",
        label: "MAKE",
        item_name: "vehicle_make",
        labels: &["make of the vehicle", "make of vehicle", "vehicle make", "car make", "auto make", "make"],
    },
    FieldSpec {
        key: "model",
        section: "VEHICLE",
        label: "MODEL",
        item_name: "vehicle_model",
        labels: &["model of the vehicle", "model of vehicle", "vehicle model", "car model", "auto model", "model"],
    },
    FieldSpec {
        key: "condition",
        section: "VEHICLE",
        label: "CONDITION",
        item_name: "condition",
        labels: &[
            "condition of the vehicle",
            "condition of vehicle",
            "vehicle condition",
            "running condition",
            "is it running",
            "does it run",
            "condition",
            "operable",
            "running",
            "runs",
        ],
    },
    FieldSpec {
        key: "trailer_type",
        section: "VEHICLE",
        label: "TRAILER TYPE",
        item_name: "transport_type",
        labels: &[
            "trailer type",
            "transport type",
            "carrier type",
            "shipping method",
            "transport method",
            "open or enclosed",
            "enclosed or open",
            "service type",
            "transport",
        ],
    },
    FieldSpec {
        key: "vehicle_type",
        section: "VEHICLE",
        label: "VEHICLE TYPE",
        item_name: "vehicle_type",
        labels: &["type of the vehicle", "type of vehicle", "vehicle type", "body type", "type"],
    },
    FieldSpec {
        key: "ship_date",
        section: "SHIPMENT",
        label: "SHIP DATE",
        item_name: "first_available_date",
        labels: &[
            "first available date",
            "available date",
            "available_date",
            "move date",
            "moving date",
            "pickup date",
            "pick up date",
            "ready date",
            "ship date",
            "shipping date",
            "transport date",
            "date of move",
        ],
    },
    FieldSpec {
        key: "notes",
        section: "SHIPMENT",
        label: "NOTES",
        item_name: "notes",
        labels: &[
            "additional information",
            "additional info",
            "additional details",
            "special instructions",
            "comments",
            "remarks",
            "comment",
            "message",
            "notes",
            "note",
        ],
    },
];

const LEGACY_ITEM_ALIASES: [(&str, &str); 1] = [("customer_name", "first_name")];

static VEHICLE_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:vehicle|unit)\s*#?\s*(\d+)\b").unwrap_or_else(|_| unreachable!()));
static TWO_LETTERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z]{2}$").unwrap_or_else(|_| unreachable!()));
static ZIP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{5}(?:-\d{4})?$").unwrap_or_else(|_| unreachable!()));
static LABELED_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^.+?:.+").unwrap_or_else(|_| unreachable!()));

pub fn field_by_key(key: &str) -> Option<&'static FieldSpec> {
    LEAD_FIELD_CATALOG.iter().find(|spec| spec.key == key)
}

pub fn field_by_item(item_name: &str) -> Option<&'static FieldSpec> {
    LEAD_FIELD_CATALOG.iter().find(|spec| spec.item_name == item_name)
}

pub fn parse_date(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim();
    if value.is_empty() {
        return None;
    }
    DATE_FORMATS
        .iter()
        .find_map(|format| {
            let date = NaiveDate::parse_from_str(value, format).ok()?;
            (!format.contains("%Y") || date.year() >= 1000).then_some(date)
        })
        .map(|date| date.format("%Y-%m-%d").to_string())
}

fn digits(value: &str) -> String {
    value.chars().filter(char::is_ascii_digit).collect()
}

fn is_valid(key: &str, value: &str) -> bool {
    match key {
        "email" => value.contains('@') && value.rsplit('@').next().is_some_and(|domain| domain.contains('.')),
        "phone" => digits(value).len() >= 7,
        "pickup_state" | "delivery_state" => TWO_LETTERS.is_match(value.trim()),
        "pickup_zip" | "delivery_zip" => ZIP.is_match(value.trim()),
        "year" => {
            let only = digits(value);
            only.len() == 4 && only.parse::<u32>().is_ok_and(|year| (1900..=2100).contains(&year))
        }
        "ship_date" => parse_date(Some(value)).is_some(),
        _ => !value.trim().is_empty(),
    }
}

pub fn field_status(key: &str, value: Option<&str>) -> &'static str {
    let value = value.unwrap_or_default().trim();
    if value.is_empty() {
        return "not_found";
    }
    if is_valid(key, value) { "matched" } else { "review" }
}

fn field_json(
    spec: &FieldSpec,
    value: Option<&str>,
    source: Option<&str>,
    matched_label: Option<&str>,
    status: &str,
    hint: &str,
) -> Value {
    json!({
        "key": spec.key,
        "section": spec.section,
        "label": spec.label,
        "item_name": spec.item_name,
        "value": value,
        "source": source,
        "matched_label": matched_label,
        "status": status,
        "hint": hint,
    })
}

pub fn blank_lead_preview() -> Value {
    let fields: Vec<Value> = LEAD_FIELD_CATALOG
        .iter()
        .map(|spec| field_json(spec, None, None, None, "not_found", "no match found in email"))
        .collect();
    json!({
        "fields": fields,
        "vehicles": [],
        "summary": {"matched": 0, "review": 0, "not_found": LEAD_FIELD_CATALOG.len(), "vehicles_detected": 0},
    })
}

pub fn extract_lines(text: Option<&str>) -> Vec<String> {
    text.unwrap_or_default()
        .trim()
        .replace("\r\n", "\n")
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty() && LABELED_LINE.is_match(line))
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ParsingValue {
    pub value: Option<String>,
    pub item_name: Option<String>,
    pub vehicle_index: Option<i64>,
}

fn lowercase_same_width(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let lower: String = ch.to_lowercase().collect();
        if lower.len() == ch.len_utf8() { out.push_str(&lower) } else { out.push(ch) }
    }
    out
}

fn custom_labels(parsing_values: &[ParsingValue]) -> (CustomLabels, IndexByLabel) {
    let mut grouped: HashMap<&'static str, Vec<String>> = HashMap::new();
    let mut index_by_label: HashMap<(&'static str, String), i64> = HashMap::new();
    for pv in parsing_values {
        let item_name = pv.item_name.as_deref().unwrap_or_default();
        let Some(field_key) = field_by_item(item_name)
            .map(|spec| spec.key)
            .or_else(|| LEGACY_ITEM_ALIASES.iter().find(|(alias, _)| *alias == item_name).map(|(_, key)| *key))
        else {
            continue;
        };
        let value = pv.value.as_deref().unwrap_or_default().trim().to_lowercase();
        if value.is_empty() {
            continue;
        }
        grouped.entry(field_key).or_default().push(value.clone());
        if let Some(index) = pv.vehicle_index.filter(|i| *i != 0) {
            index_by_label.insert((field_key, value), index);
        }
    }
    (grouped, index_by_label)
}

type CustomLabels = HashMap<&'static str, Vec<String>>;
type IndexByLabel = HashMap<(&'static str, String), i64>;

struct Hit {
    start: usize,
    end: usize,
    key: &'static str,
    index: usize,
    label: String,
}

fn scan_labels(text: &str, labels_by_key: &[(&'static str, Vec<String>)], index_by_label: &IndexByLabel) -> Vec<Hit> {
    let lowered = lowercase_same_width(text);
    let mut hits: Vec<(usize, usize, &'static str, usize, usize, String)> = Vec::new();
    for (key, labels) in labels_by_key {
        for label in labels {
            let norm = label.trim().trim_end_matches(':').trim().to_lowercase();
            if norm.is_empty() {
                continue;
            }
            let Ok(pattern) = Regex::new(&format!(r"{}\s*(\d*)\s*:", regex::escape(&norm))) else { continue };
            for found in pattern.captures_iter(&lowered) {
                let Some(whole) = found.get(0) else { continue };
                let number = found.get(1).map_or("", |m| m.as_str());
                let index: i64 = if number.is_empty() {
                    let line_start = lowered[..whole.start()].rfind('\n').map_or(0, |pos| pos + 1);
                    match VEHICLE_PREFIX.captures(&lowered[line_start..whole.start()]) {
                        Some(prefix) => prefix.get(1).and_then(|m| m.as_str().parse::<i64>().ok()).unwrap_or(1) - 1,
                        None => index_by_label.get(&(*key, norm.clone())).map_or(0, |configured| configured - 1),
                    }
                } else {
                    number.parse::<i64>().unwrap_or(1) - 1
                };
                let index = usize::try_from(index.max(0)).unwrap_or(0);
                hits.push((whole.start(), whole.end(), key, index, norm.chars().count(), norm.clone()));
            }
        }
    }
    hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.4.cmp(&a.4)));
    let mut kept = Vec::new();
    let mut last_end = 0_usize;
    let mut first = true;
    for (start, end, key, index, _, label) in hits {
        if first || start >= last_end {
            kept.push(Hit { start, end, key, index, label });
            last_end = end;
            first = false;
        }
    }
    kept
}

fn strip_value(raw: &str) -> &str {
    raw.trim().trim_matches(|c| c == ',' || c == ';').trim()
}

pub fn parse_email_fields(text: Option<&str>, parsing_values: &[ParsingValue]) -> Value {
    let text = text.unwrap_or_default();
    let (custom, index_by_label) = custom_labels(parsing_values);
    let labels_by_key: Vec<(&'static str, Vec<String>)> = LEAD_FIELD_CATALOG
        .iter()
        .map(|spec| {
            let labels = custom
                .get(spec.key)
                .filter(|labels| !labels.is_empty())
                .cloned()
                .unwrap_or_else(|| spec.labels.iter().map(|l| (*l).to_owned()).collect());
            (spec.key, labels)
        })
        .collect();
    let kept = scan_labels(text, &labels_by_key, &index_by_label);
    let vehicle_keys: BTreeSet<&str> =
        LEAD_FIELD_CATALOG.iter().filter(|spec| spec.section == "VEHICLE").map(|spec| spec.key).collect();
    let mut found: HashMap<&'static str, BTreeMap<usize, (String, String, String)>> = HashMap::new();
    let mut vehicle_slots: BTreeSet<usize> = BTreeSet::new();
    for (position, hit) in kept.iter().enumerate() {
        if vehicle_keys.contains(hit.key) {
            vehicle_slots.insert(hit.index);
        }
        let next_label = kept.get(position + 1).map_or(text.len(), |next| next.start);
        let value_end = match text[hit.end..].find('\n') {
            Some(offset) => next_label.min(hit.end + offset),
            None => next_label,
        };
        let value = strip_value(&text[hit.end..value_end]);
        if value.is_empty() {
            continue;
        }
        let line_start = text[..hit.start].rfind('\n').map_or(0, |pos| pos + 1);
        found
            .entry(hit.key)
            .or_default()
            .entry(hit.index)
            .or_insert_with(|| (value.to_owned(), text[line_start..value_end].trim().to_owned(), hit.label.clone()));
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::from([("matched", 0), ("review", 0), ("not_found", 0)]);
    let mut emit = |spec: &FieldSpec, index: usize, count: bool| -> Value {
        let (status, value) = match found.get(spec.key).and_then(|per_index| per_index.get(&index)) {
            Some((value, source, matched_label)) => {
                let status = field_status(spec.key, Some(value));
                let hint = if status == "matched" {
                    format!("matched from \"{source}\"")
                } else {
                    format!("low confidence — matched \"{value}\"")
                };
                (status, field_json(spec, Some(value), Some(source), Some(matched_label), status, &hint))
            }
            None => ("not_found", field_json(spec, None, None, None, "not_found", "no match found in email")),
        };
        if count && let Some(slot) = counts.get_mut(status) {
            *slot += 1;
        }
        value
    };
    let fields: Vec<Value> = LEAD_FIELD_CATALOG
        .iter()
        .map(|spec| {
            let index = found.get(spec.key).and_then(|per_index| per_index.keys().next().copied()).unwrap_or(0);
            emit(spec, index, true)
        })
        .collect();
    let vehicle_specs: Vec<&FieldSpec> = LEAD_FIELD_CATALOG.iter().filter(|spec| spec.section == "VEHICLE").collect();
    let vehicles: Vec<Value> = vehicle_slots
        .iter()
        .map(|idx| Value::Array(vehicle_specs.iter().map(|spec| emit(spec, *idx, false)).collect()))
        .collect();
    let mut summary = Map::new();
    summary.insert("matched".into(), json!(counts["matched"]));
    summary.insert("review".into(), json!(counts["review"]));
    summary.insert("not_found".into(), json!(counts["not_found"]));
    summary.insert("vehicles_detected".into(), json!(vehicles.len()));
    json!({"fields": fields, "vehicles": vehicles, "summary": summary})
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: &str = include_str!("../tests/fixtures/lead_fields/cases.json");
    const GOLDEN: &str = include_str!("../tests/fixtures/lead_fields/golden.json");

    #[derive(Deserialize)]
    struct Case {
        name: String,
        values: Vec<ParsingValue>,
        text: Option<String>,
    }

    fn golden() -> Map<String, Value> {
        serde_json::from_str::<Value>(GOLDEN).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default()
    }

    #[test]
    fn every_golden_case_matches_the_python_parser() {
        let cases: Vec<Case> = serde_json::from_str(CASES).unwrap_or_default();
        let golden = golden();
        assert_eq!(cases.len(), 5);
        for case in cases {
            let parsed = parse_email_fields(case.text.as_deref(), &case.values);
            assert_eq!(Some(&parsed), golden.get(&case.name), "case {}", case.name);
        }
    }

    #[test]
    fn the_blank_preview_matches_python() {
        assert_eq!(Some(&blank_lead_preview()), golden().get("_blank"));
    }

    #[test]
    fn dates_parse_like_strptime() {
        let pairs = golden().get("_dates").and_then(Value::as_array).cloned().unwrap_or_default();
        assert_eq!(pairs.len(), 18);
        for pair in pairs {
            let raw = pair.get(0).and_then(Value::as_str);
            let expected = pair.get(1).and_then(Value::as_str).map(str::to_owned);
            assert_eq!(parse_date(raw), expected, "input {raw:?}");
        }
    }

    #[test]
    fn field_statuses_match_python() {
        let triples = golden().get("_status").and_then(Value::as_array).cloned().unwrap_or_default();
        assert_eq!(triples.len(), 15);
        for triple in triples {
            let key = triple.get(0).and_then(Value::as_str).unwrap_or_default();
            let value = triple.get(1).and_then(Value::as_str);
            let expected = triple.get(2).and_then(Value::as_str).unwrap_or_default();
            assert_eq!(field_status(key, value), expected, "{key} {value:?}");
        }
    }

    #[test]
    fn labeled_lines_are_extracted() {
        assert_eq!(extract_lines(Some("a: b\r\nno colon\n : x\nc:d")), vec!["a: b", "c:d"]);
    }
}

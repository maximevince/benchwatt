//! Plain-text rendering of the JSON values `run` returns, used without `--json`.
use serde_json::Value;
use std::io::IsTerminal;

pub fn render(v: &Value) -> String {
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    render_with(v, color)
}

pub fn render_with(v: &Value, color: bool) -> String {
    if let Some(s) = v.as_str() {
        return s.into();
    }
    if v.get("rx_bytes").is_some() || v.get("stopped").is_some() {
        return String::new();
    }
    if v.get("channels").is_some() && v.get("duration_s").is_some() {
        return stats(v, color);
    }
    if let Some(props) = v.get("properties") {
        return info(props);
    }
    if let Some(output) = v.get("output").and_then(Value::as_str) {
        let mut s = format!("wrote {output}");
        if let Some(d) = v["duration_s"].as_f64() {
            s += &format!(", {}", si(d, "s"));
        }
        if let Some(samples) = v["samples"].as_object() {
            let parts: Vec<_> = samples.iter().map(|(k, n)| format!("{k} {n}")).collect();
            s += &format!(" ({} samples)", parts.join(", "));
        }
        if let Some(g) = v["gaps"].as_array().filter(|g| !g.is_empty()) {
            s += &format!(", {} gaps", g.len());
        }
        return s;
    }
    if let (Some(name), Some(value)) = (v["name"].as_str(), v["value"].as_str()) {
        return format!("{name} = {value}");
    }
    if let Some(ch) = v["channel"].as_str() {
        return format!("{ch} {}", v["value"]);
    }
    if let Some(mask) = v["gpi"].as_u64() {
        return format!("gpi1 {}  gpi2 {}", level(mask & 1), level(mask & 2));
    }
    if let Some(n) = v["gpo"].as_u64() {
        return format!("gpo{n} {}", v["state"].as_str().unwrap_or_default());
    }
    if let Some(map) = v.as_object()
        && map.keys().all(|k| {
            matches!(
                k.as_str(),
                "voltage_V" | "current_limit_A" | "mode" | "output"
            )
        })
    {
        return supply(v);
    }
    serde_json::to_string_pretty(v).unwrap()
}

fn level(bit: u64) -> &'static str {
    if bit != 0 { "high" } else { "low" }
}

fn supply(v: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(x) = v["voltage_V"].as_f64() {
        parts.push(format!("voltage {}", si(x, "V")));
    }
    if let Some(x) = v["current_limit_A"].as_f64() {
        parts.push(format!("current limit {}", si(x, "A")));
    }
    if let Some(m) = v["mode"].as_str() {
        parts.push(format!("mode {m}"));
    }
    if let Some(o) = v["output"].as_bool() {
        parts.push(format!("output {}", if o { "on" } else { "off" }));
    }
    if parts.is_empty() {
        "nothing changed".into()
    } else {
        parts.join(", ")
    }
}

fn info(props: &Value) -> String {
    let p = |k: &str| props[k].as_str().unwrap_or("?");
    let mut s = format!(
        "Otii {} · firmware {} · hardware {}\n",
        p("name"),
        p("fwversion"),
        p("hwversion")
    );
    s += &format!("serial   {}\n", p("uid"));
    s += &format!(
        "output   {}, {}, current limit {}\n",
        if p("main") == "true" { "on" } else { "off" },
        num(p("mainvolt"), "V"),
        num(p("maxcur"), "A")
    );
    s += &format!("digital  {}", num(p("expvolt"), "V"));
    s
}

fn num(s: &str, unit: &str) -> String {
    s.parse().map_or_else(|_| s.into(), |x| si(x, unit))
}

fn stats(v: &Value, color: bool) -> String {
    let paint = |code: &str, s: &str| {
        if color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.into()
        }
    };
    let mut out = Vec::new();
    let gaps = v["gaps"].as_array().map_or(0, Vec::len);
    let mut head = si(v["duration_s"].as_f64().unwrap_or(0.0), "s");
    if gaps > 0 {
        head += &format!(", {gaps} gaps");
    }
    out.push(head);
    let channels = v["channels"].as_object().cloned().unwrap_or_default();
    let cols = ["mean", "min", "max", "p_p"];
    out.push(paint(
        "2",
        &format!(
            "{:<4}{:>12}{:>12}{:>12}{:>12}",
            "", "mean", "min", "max", "p-p"
        ),
    ));
    let mut rows: Vec<_> = channels.iter().collect();
    rows.sort_by_key(|(name, _)| ORDER.iter().position(|o| o == name).unwrap_or(ORDER.len()));
    for (name, c) in rows {
        let unit = c["unit"].as_str().unwrap_or("");
        let mut row = format!("{name:<4}");
        for col in cols {
            row += &format!("{:>12}", si(c[col].as_f64().unwrap_or(f64::NAN), unit));
        }
        out.push(row);
    }
    let mut totals = Vec::new();
    if let Some(mah) = v["charge_mAh"].as_f64() {
        totals.push(format!("charge {}", si(mah * 1e-3, "Ah")));
    }
    if let Some(mwh) = v["energy"]["mwh"].as_f64() {
        totals.push(format!("energy {}", si(mwh * 1e-3, "Wh")));
    }
    if let Some(w) = v["energy"]["average_power"].as_f64() {
        totals.push(format!("average power {}", si(w, "W")));
    }
    if !totals.is_empty() {
        out.push(totals.join(" · "));
    }
    for a in v["assertions"].as_array().into_iter().flatten() {
        let expr = a["expression"].as_str().unwrap_or("");
        let unit = assertion_unit(expr, &channels);
        let actual = si(a["actual"].as_f64().unwrap_or(f64::NAN), unit);
        let mark = if a["passed"].as_bool() == Some(true) {
            paint("32", "PASS")
        } else {
            paint("31", "FAIL")
        };
        out.push(format!("{mark}  {expr}  (was {actual})"));
    }
    out.join("\n")
}

/// Same order as the app's channel list.
const ORDER: [&str; 10] = ["mc", "mv", "mp", "ac", "av", "ap", "vb", "vj", "tp", "sn"];

fn assertion_unit<'a>(expr: &str, channels: &'a serde_json::Map<String, Value>) -> &'a str {
    let target = expr.split(['<', '>']).next().unwrap_or("").trim();
    match target {
        "charge" => "Ah",
        "energy" => "Wh",
        _ => target
            .split_once('.')
            .and_then(|(ch, _)| channels.get(ch))
            .and_then(|c| c["unit"].as_str())
            .unwrap_or(""),
    }
}

/// Four significant digits with an SI prefix: `si(0.01799, "A")` is "17.99 mA".
pub fn si(x: f64, unit: &str) -> String {
    if !x.is_finite() {
        return "–".into();
    }
    if x == 0.0 {
        return format!("0 {unit}");
    }
    let (scale, prefix) = [(1.0, ""), (1e-3, "m"), (1e-6, "µ"), (1e-9, "n")]
        .into_iter()
        .find(|(s, _)| x.abs() >= *s * 0.99995)
        .unwrap_or((1e-9, "n"));
    let y = x / scale;
    let decimals = (3 - y.abs().log10().floor() as i32).clamp(0, 3) as usize;
    format!("{y:.decimals$} {prefix}{unit}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn prefixes() {
        assert_eq!(si(0.017994, "A"), "17.99 mA");
        assert_eq!(si(1.8, "V"), "1.800 V");
        assert_eq!(si(13.9e-6, "A"), "13.90 µA");
        assert_eq!(si(0.0, "A"), "0 A");
        assert_eq!(si(2.001, "s"), "2.001 s");
        assert_eq!(si(1234.0, "s"), "1234 s");
    }
    #[test]
    fn measure_table() {
        let v = json!({"duration_s":1.0,"gaps":[],"charge_mAh":0.005,"energy":null,
            "channels":{"mc":{"unit":"A","mean":0.018,"min":0.0179,"max":0.0181,"p_p":0.0002}},
            "assertions":[{"expression":"mc.mean<10mA","actual":0.018,"limit":0.01,"passed":false}]});
        let s = render_with(&v, false);
        assert!(s.contains("mc      18.00 mA"), "{s}");
        assert!(s.contains("charge 5.000 µAh"), "{s}");
        assert!(s.ends_with("FAIL  mc.mean<10mA  (was 18.00 mA)"), "{s}");
    }
    #[test]
    fn supply_summary() {
        let v = json!({"voltage_V":1.8,"current_limit_A":0.05,"output":true});
        assert_eq!(
            render_with(&v, false),
            "voltage 1.800 V, current limit 50.00 mA, output on"
        );
    }
}

use bot_core::types::{ContractSpec, ContractType, SymbolId};

/// Parse active_symbols response and filter for tradeable symbols.
pub fn parse_active_symbols(json: &serde_json::Value) -> Vec<SymbolId> {
    let mut symbols = Vec::new();

    if let Some(arr) = json.get("active_symbols").and_then(|a| a.as_array()) {
        for sym in arr {
            let symbol = sym.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            let market = sym.get("market").and_then(|m| m.as_str()).unwrap_or("");
            let submarket = sym.get("submarket").and_then(|s| s.as_str()).unwrap_or("");

            // Include volatility indices, crash/boom, forex, and crypto
            let include = matches!(market, "synthetic_index" | "forex" | "cryptocurrency")
                || submarket.contains("random")
                || submarket.contains("continuous");

            if include && !symbol.is_empty() {
                symbols.push(SymbolId(symbol.to_string()));
            }
        }
    }

    symbols
}

/// Parse contracts_for response and build ContractSpec entries.
pub fn parse_contracts_for(symbol: &SymbolId, json: &serde_json::Value) -> Vec<ContractSpec> {
    let mut specs = Vec::new();

    let available = json
        .get("contracts_for")
        .and_then(|c| c.get("available"))
        .and_then(|a| a.as_array());

    let Some(contracts) = available else { return specs; };

    for c in contracts {
        let ct = c.get("contract_type").and_then(|t| t.as_str()).unwrap_or("");
        let display = c.get("contract_display").and_then(|d| d.as_str())
            .or_else(|| c.get("contract_category_display").and_then(|d| d.as_str()))
            .unwrap_or("")
            .to_string();

        // We only want Rise/Fall (CALL/PUT)
        let contract_type = match ct {
            "CALL" => ContractType::Call,
            "PUT" => ContractType::Put,
            _ => continue,
        };

        // Extract min/max duration
        let min_duration = c.get("min_contract_duration").and_then(|d| d.as_str()).unwrap_or("5t");
        let _max_duration = c.get("max_contract_duration").and_then(|d| d.as_str()).unwrap_or("365d");

        // Parse a sensible default duration (5 minutes = 300s)
        let duration_sec = parse_duration_to_sec(min_duration).max(300);

        specs.push(ContractSpec {
            symbol: symbol.clone(),
            display_name: display,
            contract_type,
            duration_sec,
            payout_multiplier: 1.95, // typical for Rise/Fall
        });
    }

    specs
}

/// Convert Deriv duration string (e.g. "5t", "15s", "5m", "1h", "365d") to seconds.
fn parse_duration_to_sec(dur: &str) -> u64 {
    let dur = dur.trim();
    if dur.is_empty() { return 300; }

    let (num_str, unit) = dur.split_at(dur.len() - 1);
    let num: u64 = num_str.parse().unwrap_or(5);

    match unit {
        "t" => num * 2, // ~2s per tick on synthetics (rough approximation)
        "s" => num,
        "m" => num * 60,
        "h" => num * 3600,
        "d" => num * 86400,
        _ => 300,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration_to_sec("5t"), 10);
        assert_eq!(parse_duration_to_sec("15s"), 15);
        assert_eq!(parse_duration_to_sec("5m"), 300);
        assert_eq!(parse_duration_to_sec("1h"), 3600);
        assert_eq!(parse_duration_to_sec("365d"), 31536000);
    }

    #[test]
    fn test_parse_active_symbols() {
        let json = serde_json::json!({
            "active_symbols": [
                {"symbol": "R_100", "market": "synthetic_index", "submarket": "random_index"},
                {"symbol": "frxEURUSD", "market": "forex", "submarket": "major_pairs"},
                {"symbol": "WLDAUD", "market": "stocks", "submarket": "us_stocks"},
            ]
        });
        let symbols = parse_active_symbols(&json);
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].0, "R_100");
        assert_eq!(symbols[1].0, "frxEURUSD");
    }
}

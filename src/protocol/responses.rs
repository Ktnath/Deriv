use crate::types::TickUpdate;
use serde_json::Value;

/// Parsed API response.
#[derive(Debug, Clone)]
pub enum DerivResponse {
    Authorize {
        balance: f64,
        currency: String,
        login_id: String,
    },
    Tick(TickUpdate),
    Proposal {
        req_id: u64,
        proposal_id: String,
        ask_price: f64,
        payout: f64,
    },
    Buy {
        contract_id: String,
        buy_price: f64,
        payout: f64,
    },
    ProposalOpenContract {
        contract_id: String,
        status: String, // "open", "sold", "expired"
        profit: f64,
        buy_price: f64,
        current_spot: f64,
        is_sold: bool,
        is_expired: bool,
        is_valid_to_sell: bool,
        subscription_id: Option<String>,
    },
    Sell {
        contract_id: String,
        sold_for: f64,
        profit: f64,
    },
    Balance {
        balance: f64,
        currency: String,
    },
    Time {
        server_time: i64,
    },
    Forget {
        count: u64,
    },
    Error {
        code: String,
        message: String,
        req_id: Option<u64>,
    },
    Unknown(Value),
}

/// Extract error from a response, if present.
pub fn extract_error(v: &Value) -> Option<(String, String)> {
    v.get("error").map(|err| {
        let code = err
            .get("code")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        (code, message)
    })
}

/// Extract msg_type from response.
pub fn extract_msg_type(v: &Value) -> &str {
    v.get("msg_type").and_then(|m| m.as_str()).unwrap_or("")
}

fn parse_f64_field(obj: &Value, field: &str) -> f64 {
    obj.get(field)
        .and_then(|v| v.as_f64())
        .or_else(|| {
            obj.get(field)
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0.0)
}

/// Parse a raw JSON message into a typed DerivResponse.
pub fn parse_response(text: &str) -> DerivResponse {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return DerivResponse::Unknown(Value::Null),
    };

    // Check error first
    if let Some((code, message)) = extract_error(&v) {
        let req_id = v.get("req_id").and_then(|r| r.as_u64());
        return DerivResponse::Error {
            code,
            message,
            req_id,
        };
    }

    match extract_msg_type(&v) {
        "authorize" => {
            let auth = &v["authorize"];
            DerivResponse::Authorize {
                balance: parse_f64_field(auth, "balance"),
                currency: auth
                    .get("currency")
                    .and_then(|c| c.as_str())
                    .unwrap_or("USD")
                    .to_string(),
                login_id: auth
                    .get("loginid")
                    .and_then(|l| l.as_str())
                    .unwrap_or("")
                    .to_string(),
            }
        }
        "tick" => {
            let tick = &v["tick"];
            DerivResponse::Tick(TickUpdate {
                symbol: tick
                    .get("symbol")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
                price: parse_f64_field(tick, "quote"),
                epoch: tick.get("epoch").and_then(|e| e.as_i64()).unwrap_or(0),
            })
        }
        "proposal" => {
            let prop = &v["proposal"];
            DerivResponse::Proposal {
                req_id: v.get("req_id").and_then(|r| r.as_u64()).unwrap_or(0),
                proposal_id: prop
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string(),
                ask_price: parse_f64_field(prop, "ask_price"),
                payout: parse_f64_field(prop, "payout"),
            }
        }
        "buy" => {
            let buy = &v["buy"];
            DerivResponse::Buy {
                contract_id: buy
                    .get("contract_id")
                    .and_then(|c| c.as_u64())
                    .map(|c| c.to_string())
                    .or_else(|| {
                        buy.get("contract_id")
                            .and_then(|c| c.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default(),
                buy_price: parse_f64_field(buy, "buy_price"),
                payout: parse_f64_field(buy, "payout"),
            }
        }
        "proposal_open_contract" => {
            let poc = &v["proposal_open_contract"];
            let sub_id = v
                .get("subscription")
                .and_then(|s| s.get("id"))
                .and_then(|i| i.as_str())
                .map(|s| s.to_string());
            DerivResponse::ProposalOpenContract {
                contract_id: poc
                    .get("contract_id")
                    .and_then(|c| c.as_u64())
                    .map(|c| c.to_string())
                    .or_else(|| {
                        poc.get("contract_id")
                            .and_then(|c| c.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default(),
                status: poc
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                profit: parse_f64_field(poc, "profit"),
                buy_price: parse_f64_field(poc, "buy_price"),
                current_spot: parse_f64_field(poc, "current_spot"),
                is_sold: poc.get("is_sold").and_then(|v| v.as_u64()).unwrap_or(0) == 1,
                is_expired: poc.get("is_expired").and_then(|v| v.as_u64()).unwrap_or(0) == 1,
                is_valid_to_sell: poc
                    .get("is_valid_to_sell")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    == 1,
                subscription_id: sub_id,
            }
        }
        "balance" => {
            let bal = &v["balance"];
            DerivResponse::Balance {
                balance: parse_f64_field(bal, "balance"),
                currency: bal
                    .get("currency")
                    .and_then(|c| c.as_str())
                    .unwrap_or("USD")
                    .to_string(),
            }
        }
        "time" => DerivResponse::Time {
            server_time: v.get("time").and_then(|t| t.as_i64()).unwrap_or(0),
        },
        "sell" => {
            let sell = &v["sell"];
            DerivResponse::Sell {
                contract_id: sell
                    .get("contract_id")
                    .and_then(|c| c.as_u64())
                    .map(|c| c.to_string())
                    .or_else(|| {
                        sell.get("contract_id")
                            .and_then(|c| c.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default(),
                sold_for: parse_f64_field(sell, "sold_for"),
                profit: parse_f64_field(sell, "profit"),
            }
        }
        "forget" | "forget_all" => DerivResponse::Forget {
            count: v.get("forget").and_then(|f| f.as_u64()).unwrap_or(0),
        },
        _ => DerivResponse::Unknown(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tick() {
        let json =
            r#"{"msg_type":"tick","tick":{"symbol":"R_100","quote":1234.56,"epoch":1700000000}}"#;
        match parse_response(json) {
            DerivResponse::Tick(t) => {
                assert_eq!(t.symbol, "R_100");
                assert!((t.price - 1234.56).abs() < 0.001);
                assert_eq!(t.epoch, 1700000000);
            }
            other => panic!("Expected Tick, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_error() {
        let json =
            r#"{"error":{"code":"AuthorizationRequired","message":"Please log in"},"req_id":42}"#;
        match parse_response(json) {
            DerivResponse::Error {
                code,
                message,
                req_id,
            } => {
                assert_eq!(code, "AuthorizationRequired");
                assert_eq!(message, "Please log in");
                assert_eq!(req_id, Some(42));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_proposal_open_contract() {
        let json = r#"{"msg_type":"proposal_open_contract","proposal_open_contract":{"contract_id":123,"status":"open","profit":0.5,"buy_price":1.0,"current_spot":100.0,"is_sold":0,"is_expired":0,"is_valid_to_sell":1},"subscription":{"id":"sub_abc"}}"#;
        match parse_response(json) {
            DerivResponse::ProposalOpenContract {
                contract_id,
                status,
                is_sold,
                subscription_id,
                ..
            } => {
                assert_eq!(contract_id, "123");
                assert_eq!(status, "open");
                assert!(!is_sold);
                assert_eq!(subscription_id, Some("sub_abc".to_string()));
            }
            other => panic!("Expected POC, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sell_response() {
        let json =
            r#"{"msg_type":"sell","sell":{"contract_id":456,"sold_for":1.85,"profit":0.35}}"#;
        match parse_response(json) {
            DerivResponse::Sell {
                contract_id,
                sold_for,
                profit,
            } => {
                assert_eq!(contract_id, "456");
                assert!((sold_for - 1.85).abs() < 0.001);
                assert!((profit - 0.35).abs() < 0.001);
            }
            other => panic!("Expected Sell, got {:?}", other),
        }
    }
}

use serde_json::Value;

/// Build authorize request.
pub fn authorize(token: &str) -> Value {
    serde_json::json!({ "authorize": token })
}

/// Build ticks subscription request.
pub fn ticks(symbol: &str) -> Value {
    serde_json::json!({ "ticks": symbol, "subscribe": 1 })
}

/// Build proposal request (get price for a contract).
pub fn proposal(
    symbol: &str,
    contract_type: &str,
    duration: u64,
    duration_unit: &str,
    stake: f64,
) -> Value {
    serde_json::json!({
        "proposal": 1,
        "amount": stake,
        "basis": "stake",
        "contract_type": contract_type,
        "currency": "USD",
        "duration": duration,
        "duration_unit": duration_unit,
        "symbol": symbol
    })
}

/// Build buy request using a proposal ID.
pub fn buy(proposal_id: &str, price: f64) -> Value {
    serde_json::json!({ "buy": proposal_id, "price": price })
}

/// Build proposal_open_contract subscription (monitor a contract).
pub fn proposal_open_contract(contract_id: &str) -> Value {
    serde_json::json!({ "proposal_open_contract": 1, "contract_id": contract_id, "subscribe": 1 })
}

/// Build a one-shot proposal_open_contract request (no subscription).
/// Used for active polling to reduce SL/TP slippage.
pub fn proposal_open_contract_poll(contract_id: &str) -> Value {
    serde_json::json!({ "proposal_open_contract": 1, "contract_id": contract_id })
}

/// Build balance subscription request.
pub fn balance_subscribe() -> Value {
    serde_json::json!({ "balance": 1, "subscribe": 1 })
}

/// Build sell request (early exit from a contract).
pub fn sell(contract_id: &str, price: f64) -> Value {
    serde_json::json!({ "sell": contract_id, "price": price })
}

/// Build forget request.
pub fn forget(subscription_id: &str) -> Value {
    serde_json::json!({ "forget": subscription_id })
}

/// Build forget_all request for a msg_type.
pub fn forget_all(msg_type: &str) -> Value {
    serde_json::json!({ "forget_all": msg_type })
}

/// Build time request (ping/health).
pub fn time() -> Value {
    serde_json::json!({ "time": 1 })
}

/// Build logout request.
pub fn logout() -> Value {
    serde_json::json!({ "logout": 1 })
}

/// Build active_symbols request.
pub fn active_symbols() -> Value {
    serde_json::json!({ "active_symbols": "brief", "product_type": "basic" })
}

/// Build contracts_for request.
pub fn contracts_for(symbol: &str) -> Value {
    serde_json::json!({ "contracts_for": symbol, "currency": "USD", "product_type": "basic" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authorize_request() {
        let req = authorize("my_token");
        assert_eq!(req["authorize"], "my_token");
    }

    #[test]
    fn test_proposal_request() {
        let req = proposal("R_100", "CALL", 300, "s", 1.5);
        assert_eq!(req["proposal"], 1);
        assert_eq!(req["symbol"], "R_100");
        assert_eq!(req["contract_type"], "CALL");
        assert_eq!(req["duration"], 300);
        assert_eq!(req["amount"], 1.5);
    }

    #[test]
    fn test_buy_request() {
        let req = buy("abc123", 0.95);
        assert_eq!(req["buy"], "abc123");
        assert_eq!(req["price"], 0.95);
    }

    #[test]
    fn test_proposal_open_contract_request() {
        let req = proposal_open_contract("cid_456");
        assert_eq!(req["contract_id"], "cid_456");
        assert_eq!(req["subscribe"], 1);
    }

    #[test]
    fn test_sell_request() {
        let req = sell("c_12345", 5.50);
        assert_eq!(req["sell"], "c_12345");
        assert_eq!(req["price"], 5.50);
    }
}

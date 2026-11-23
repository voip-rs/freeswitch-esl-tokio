//! Gateway registration state and ping status enums.

wire_enum! {
    /// Gateway registration state from `sofia::gateway_state` events.
    ///
    /// The `State` header value, mapping to `reg_state_t` / `sofia_state_names[]`
    /// in mod_sofia.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum GatewayRegState {
        Unreged => "UNREGED",
        Trying => "TRYING",
        Register => "REGISTER",
        Reged => "REGED",
        Unregister => "UNREGISTER",
        Failed => "FAILED",
        FailWait => "FAIL_WAIT",
        Expired => "EXPIRED",
        Noreg => "NOREG",
        Down => "DOWN",
        Timeout => "TIMEOUT",
    }
    error ParseGatewayRegStateError("gateway reg state");
    tests: gateway_reg_state_wire_tests;
}

wire_enum! {
    /// Gateway ping status from `sofia::gateway_state` events.
    ///
    /// The `Ping-Status` header value, mapping to `sofia_gateway_status_name()`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum GatewayPingStatus {
        Down => "DOWN",
        Up => "UP",
        Invalid => "INVALID",
    }
    error ParseGatewayPingStatusError("gateway ping status");
    tests: gateway_ping_status_wire_tests;
}

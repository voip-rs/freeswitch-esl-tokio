use super::check_expression_field;
use crate::commands::originate::OriginateError;
use crate::commands::variables::Variables;

wire_enum! {
    /// Distribution order for group_call dial strings.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum GroupCallOrder {
        /// Ring all members simultaneously.
        All => "A",
        /// Enterprise-style hunt (try each in order, across domains).
        Enterprise => "E",
        /// Ring first available member only.
        First => "F",
    }
    error ParseGroupCallOrderError("group call order");
    tests: group_call_order_tests;
}

/// Runtime expression resolving directory group members:
/// `${group_call(group@domain[+order])}`. Parse one with
/// [`Endpoint::parse_for`](super::Endpoint::parse_for) at
/// [`DialStringCarrier::Dialplan`](crate::commands::variables::DialStringCarrier::Dialplan).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "config::GroupCall"))]
pub struct GroupCall {
    /// Group name from the directory.
    group: String,
    /// Domain for the group lookup.
    domain: String,
    /// Distribution order.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    order: Option<GroupCallOrder>,
    /// Per-channel variables prepended as `{key=value}`.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    variables: Option<Variables>,
}

impl GroupCall {
    /// Create a new group_call expression, refusing what parse refuses.
    pub fn new(
        group: impl Into<String>,
        domain: impl Into<String>,
    ) -> Result<Self, OriginateError> {
        Self {
            group: group.into(),
            domain: domain.into(),
            order: None,
            variables: None,
        }
        .checked()
    }

    /// Set the distribution order.
    pub fn with_order(mut self, order: GroupCallOrder) -> Self {
        self.order = Some(order);
        self
    }

    /// Group name from the directory.
    pub fn group(&self) -> &str {
        &self.group
    }

    /// Domain for the group lookup.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// Distribution order, `None` to omit the `+order` suffix.
    pub fn order(&self) -> Option<GroupCallOrder> {
        self.order
    }
}

impl_dial_string_with_variables!(GroupCall, write_expression, |this| match &this.order {
    Some(o) => format!("${{group_call({}@{}+{o})}}", this.group, this.domain),
    None => format!("${{group_call({}@{})}}", this.group, this.domain),
});

impl GroupCall {
    /// Refuse what `group_call_function` splits, the order at its first `+` and then the domain
    /// at `@`, and what a pass ahead of it reads.
    pub(crate) fn check_deliverable(&self) -> Result<(), OriginateError> {
        const KIND: &str = "group_call";
        check_expression_field(KIND, "group", &self.group, &["+", "@"])?;
        check_expression_field(KIND, "domain", &self.domain, &["+"])
    }

    pub(crate) fn parse_bare(text: &str) -> Result<Self, OriginateError> {
        let inner = text
            .strip_prefix("${group_call(")
            .and_then(|r| r.strip_suffix(")}"))
            .ok_or_else(|| OriginateError::ParseError("not a group_call expression".into()))?;
        let (group_at_domain, order) = if let Some((gd, o)) = inner.split_once('+') {
            let order: GroupCallOrder = o
                .parse()
                .map_err(|source| OriginateError::UnknownGroupCallOrder {
                    value: o.to_string(),
                    source,
                })?;
            (gd, Some(order))
        } else {
            (inner, None)
        };
        let (group, domain) = group_at_domain
            .split_once('@')
            .ok_or_else(|| OriginateError::ParseError("group_call needs group@domain".into()))?;
        let ep = Self {
            group: group.into(),
            domain: domain.into(),
            order,
            variables: None,
        };
        ep.check_deliverable()?;
        Ok(ep)
    }
}

impl_endpoint_parse!(config:
    GroupCall {
        group: String,
        domain: String,
        ;
        order: Option<GroupCallOrder>,
        variables: Option<Variables>,
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_call_display() {
        let ep = GroupCall::new("support", "example.com").unwrap();
        assert_eq!(ep.to_string(), "${group_call(support@example.com)}");
    }

    #[test]
    fn group_call_display_with_order() {
        let ep = GroupCall::new("support", "example.com")
            .unwrap()
            .with_order(GroupCallOrder::All);
        assert_eq!(ep.to_string(), "${group_call(support@example.com+A)}");
    }

    #[test]
    fn group_call_from_str() {
        let ep = GroupCall::parse_bare("${group_call(support@example.com)}").unwrap();
        assert_eq!(ep.group, "support");
        assert_eq!(ep.domain, "example.com");
        assert!(ep
            .order
            .is_none());
    }

    #[test]
    fn group_call_from_str_with_order() {
        let ep = GroupCall::parse_bare("${group_call(support@example.com+A)}").unwrap();
        assert_eq!(ep.group, "support");
        assert_eq!(ep.domain, "example.com");
        assert_eq!(ep.order, Some(GroupCallOrder::All));
    }

    #[test]
    fn group_call_round_trip() {
        let ep = GroupCall::new("calltakers", "example.com")
            .unwrap()
            .with_order(GroupCallOrder::Enterprise);
        let s = ep.to_string();
        let parsed = GroupCall::parse_bare(&s).unwrap();
        assert_eq!(parsed, ep);
    }

    #[test]
    fn serde_group_call() {
        let ep = GroupCall::new("support", "example.com")
            .unwrap()
            .with_order(GroupCallOrder::All);
        let json = serde_json::to_string(&ep).unwrap();
        let parsed: GroupCall = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ep);
    }
}

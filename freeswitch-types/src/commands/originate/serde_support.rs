use super::*;
use crate::switch_passes::originate_function::check_target_readable;

/// Intermediate type for serde, mirroring the old public-field layout.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct OriginateRaw {
    pub endpoint: Endpoint,
    #[serde(flatten)]
    pub target: OriginateTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialplan: Option<DialplanType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid_num: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv_separator: Option<char>,
}

impl TryFrom<OriginateRaw> for Originate {
    type Error = OriginateError;

    fn try_from(raw: OriginateRaw) -> Result<Self, Self::Error> {
        let dialplan = raw.dialplan;
        raw.endpoint
            .check_expanded(DialStringCarrier::EslApi)?;
        check_dialplan_fits(&raw.target, dialplan.as_ref())?;
        if let OriginateTarget::InlineApplications(ref apps) = raw.target {
            if apps.is_empty() {
                return Err(OriginateError::EmptyInlineApplications);
            }
        }
        for (field, value) in [
            ("context", &raw.context),
            ("cid_name", &raw.cid_name),
            ("cid_num", &raw.cid_num),
        ] {
            if value
                .as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case(UNDEF))
            {
                return Err(OriginateError::UndefPositional(field));
            }
        }
        let originate = Self {
            endpoint: raw.endpoint,
            target: raw.target,
            dialplan,
            context: raw.context,
            cid_name: raw.cid_name,
            cid_num: raw.cid_num,
            timeout: raw
                .timeout_secs
                .map(Duration::from_secs),
            // A config file names applications, not a wire separator.
            inline_delimiter: None,
            argv_separator: None,
        };
        if originate
            .target_text()
            .eq_ignore_ascii_case(UNDEF)
        {
            return Err(OriginateError::UndefPositional("target"));
        }
        check_target_readable(&originate.target)?;
        match raw.argv_separator {
            Some(sep) => originate.with_argv_separator(sep),
            None => Ok(originate),
        }
    }
}

impl From<Originate> for OriginateRaw {
    fn from(o: Originate) -> Self {
        Self {
            endpoint: o.endpoint,
            target: o.target,
            dialplan: o.dialplan,
            context: o.context,
            cid_name: o.cid_name,
            cid_num: o.cid_num,
            timeout_secs: o
                .timeout
                .map(|d| d.as_secs()),
            argv_separator: o.argv_separator,
        }
    }
}

impl serde::Serialize for Originate {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        OriginateRaw::from(self.clone()).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Originate {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = OriginateRaw::deserialize(deserializer)?;
        Originate::try_from(raw).map_err(serde::de::Error::custom)
    }
}

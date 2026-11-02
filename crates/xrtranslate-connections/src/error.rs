use std::fmt;

/// Connection errors never contain response bodies, authorization URLs or tokens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionError {
    Cancelled,
    Storage,
    Network,
    InvalidAuthorization,
    LoginRequired,
    PlanRequired,
    NoModels,
    ModelUnavailable,
    RemoteRevocationUnconfirmed,
}
impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "Sign-in cancelled.",
            Self::Storage => "Cannot read or save the account connection. Sign in again.",
            Self::Network => "Could not reach the account service. Check your connection and try again.",
            Self::InvalidAuthorization => "Account authorization could not be verified. Please try signing in again.",
            Self::LoginRequired => "Sign in again on the Welcome Page.",
            Self::PlanRequired => "Allow plan usage when signing in. An eligible account plan is required.",
            Self::NoModels => "No models are available for this account. Check your plan and app permissions.",
            Self::ModelUnavailable => "This model is unavailable for the selected account. Refresh models on the Welcome Page.",
            Self::RemoteRevocationUnconfirmed => "Signed out locally. The service could not confirm revocation; review XRTranslate access in your account settings.",
        })
    }
}
impl std::error::Error for ConnectionError {}

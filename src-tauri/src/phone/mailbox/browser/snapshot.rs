//! Web browser snapshot: the serializable `WebBrowserSnapshot` state record.
//! Holds `url`, `title`, `loading` and `visible`, serialized in camelCase.
//! Plain data only; it does not drive or observe a browser itself.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebBrowserSnapshot {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub visible: bool,
}

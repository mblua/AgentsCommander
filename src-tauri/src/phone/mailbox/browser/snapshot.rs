use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebBrowserSnapshot {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub visible: bool,
}

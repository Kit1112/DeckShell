//! Native preflight, explicit EULA acceptance and background install dispatch.
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    pub required: Option<u64>,
    pub available: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceStatus { Unknown, Insufficient, Sufficient }

fn safe_bytes(v: &Value) -> Option<u64> {
    v.as_u64().filter(|n| *n <= 9_007_199_254_740_991)
}

impl Space {
    pub fn from_manager(manager: &Value) -> Self {
        Self {
            required: manager.get("nDiskSpaceRequired").and_then(safe_bytes),
            available: manager.get("nDiskSpaceAvailable").and_then(safe_bytes),
        }
    }

    pub fn status(self) -> SpaceStatus {
        match (self.required, self.available) {
            // The native picker permits an exact fit; Steam validates again at dispatch.
            (Some(required), Some(available)) if required > available => SpaceStatus::Insufficient,
            (Some(_), Some(_)) => SpaceStatus::Sufficient,
            _ => SpaceStatus::Unknown,
        }
    }
}

/// Validate ownership against the complete native request, rather than merely its
/// currentAppID (multi-app requests are intentionally unsupported in this increment).
pub fn is_single_app_request(manager: &Value, appid: u32) -> bool {
    appid != 0 && manager.get("rgApps").and_then(Value::as_array).is_some_and(|apps| {
        apps.len() == 1 && apps[0].get("nAppID").and_then(Value::as_u64) == Some(appid.into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absent_invalid_and_partial_space_remain_unknown() {
        for v in [json!(null), json!({}), json!({"nDiskSpaceRequired": 100}),
            json!({"nDiskSpaceRequired": -1, "nDiskSpaceAvailable": 200}),
            json!({"nDiskSpaceRequired": "100", "nDiskSpaceAvailable": 200})] {
            assert_eq!(Space::from_manager(&v).status(), SpaceStatus::Unknown);
        }
        assert_eq!(Space::from_manager(&json!({})).available, None);
    }

    #[test]
    fn no_space_blocks_and_exact_fit_is_sufficient() {
        for available in [0, 99] {
            assert_eq!(Space { required: Some(100), available: Some(available) }.status(), SpaceStatus::Insufficient);
        }
        assert_eq!(Space { required: Some(100), available: Some(100) }.status(), SpaceStatus::Sufficient);
    }

    #[test]
    fn ownership_rejects_foreign_unknown_and_multi_app_requests() {
        assert!(is_single_app_request(&json!({"rgApps":[{"nAppID":42}]}), 42));
        for v in [json!(null), json!({"currentAppID":42}), json!({"rgApps":[]}),
            json!({"rgApps":[{"nAppID":43}]}), json!({"rgApps":[{"nAppID":42},{"nAppID":43}]})] {
            assert!(!is_single_app_request(&v, 42));
        }
        assert!(!is_single_app_request(&json!({"rgApps":[{"nAppID":0}]}), 0));
    }
}

/// Native library folders. Steam performs the final write/space check at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFolder {
    pub index: u32,
    pub drive_name: String,
    pub user_label: Option<String>,
    pub mounted: Option<bool>,
    pub default: Option<bool>,
    pub free: Option<u64>,
}

pub fn native_folders(value: &Value) -> Result<Vec<NativeFolder>, &'static str> {
    let array = value.as_array().ok_or("native folder list unavailable")?;
    let mut seen = std::collections::HashSet::new();
    array.iter().map(|v| {
        let index = v.get("nFolderIndex").and_then(Value::as_u64)
            .and_then(|i| u32::try_from(i).ok()).ok_or("invalid native folder index")?;
        if !seen.insert(index) { return Err("duplicate native folder index"); }
        let drive_name = v.get("strDriveName").and_then(Value::as_str)
            .filter(|s| !s.is_empty()).ok_or("native folder name unavailable")?.to_owned();
        Ok(NativeFolder {
            index, drive_name,
            user_label: v.get("strUserLabel").and_then(Value::as_str).map(str::to_owned),
            mounted: v.get("bIsMounted").and_then(Value::as_bool),
            default: v.get("bIsDefaultFolder").and_then(Value::as_bool),
            // Steam JS numbers must be exactly representable; strings/BigInt
            // transport conversions are unproven and must remain unknown.
            free: v.get("nFreeSpace").and_then(Value::as_u64).filter(|n| *n <= 9_007_199_254_740_991),
        })
    }).collect()
}

#[cfg(test)]
mod folder_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_install_folders_preserve_ids_and_unknown_without_path_guesses() {
        let list = native_folders(&json!([
            {"nFolderIndex":7,"strDriveName":"/native/library","bIsMounted":true,"bIsDefaultFolder":true,"nFreeSpace":0},
            {"nFolderIndex":19,"strDriveName":"/native/removable","nFreeSpace":"100"}
        ])).unwrap();
        assert_eq!(list[0].index, 7); assert_eq!(list[0].free, Some(0));
        assert_eq!(list[1].mounted, None); assert_eq!(list[1].default, None); assert_eq!(list[1].free, None);
        // DefaultInstallFolderIndex in stock uses findIndex, which is 0 here,
        // NOT nFolderIndex 7. Caller must resolve this before enabling skip.
        assert_eq!(list.iter().position(|f| f.default == Some(true)), Some(0));
    }
    #[test]
    fn native_install_folders_reject_bad_ids_missing_list_and_duplicates() {
        for v in [json!(null), json!({}), json!([{}]), json!([{"nFolderIndex":-1,"strDriveName":"a"}]),
            json!([{"nFolderIndex":0,"strDriveName":"a"},{"nFolderIndex":0,"strDriveName":"b"}])] {
            assert!(native_folders(&v).is_err());
        }
        let list = native_folders(&json!([{"nFolderIndex":0,"strDriveName":"a","nFreeSpace":9_007_199_254_740_992_u64}])).unwrap();
        assert_eq!(list[0].free, None);
    }
}

#[derive(Debug, Clone)]
pub struct Eula { pub id: String, pub version: u32, pub url: String }
#[derive(Debug, Clone)]
pub struct PreflightData { pub folders: Vec<NativeFolder>, pub required: Option<u64>, pub eulas: Vec<Eula> }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckCategory {
    Unknown,
    Unsupported,
    Playable,
    Verified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckTestResult {
    Unknown,
    Note,
    Fail,
    Warning,
    Pass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckCompatItem {
    pub result: DeckTestResult,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeckCompatibility {
    pub category: Option<DeckCategory>,
    pub items: Option<Vec<DeckCompatItem>>,
    pub blog_url: Option<String>,
}

impl DeckCompatibility {
    fn from_response(value: &Value) -> Self {
        Self {
            category: match value["resolved_category"].as_u64() {
                Some(0) => Some(DeckCategory::Unknown),
                Some(1) => Some(DeckCategory::Unsupported),
                Some(2) => Some(DeckCategory::Playable),
                Some(3) => Some(DeckCategory::Verified),
                _ => None,
            },
            items: value["resolved_items"].as_array().map(|items| {
                items.iter().map(|item| DeckCompatItem {
                    result: match item["display_type"].as_u64() {
                        Some(1) => DeckTestResult::Note,
                        Some(2) => DeckTestResult::Fail,
                        Some(3) => DeckTestResult::Warning,
                        Some(4) => DeckTestResult::Pass,
                        _ => DeckTestResult::Unknown,
                    },
                    token: item["loc_token"].as_str()
                        .filter(|token| !token.trim().is_empty())
                        .map(str::to_owned),
                }).collect()
            }),
            blog_url: value["steam_deck_blog_url"].as_str()
                .filter(|url| url.starts_with("https://") && url.len() > "https://".len())
                .map(str::to_owned),
        }
    }
}

pub const PREFLIGHT_JS: &str = include_str!("install_preflight.js");
impl crate::Bridge {
    pub fn install_deck_compatibility(&self, appid: u32) -> crate::Result<DeckCompatibility> {
        let script = include_str!("install_compatibility.js").replace("{APPID}", &appid.to_string());
        let value = self.eval_own(&script, std::time::Duration::from_secs(5))?;
        Ok(DeckCompatibility::from_response(&value))
    }

    pub fn install_preflight(&self, appid: u32) -> crate::Result<PreflightData> {
        let value = self.eval_own(&PREFLIGHT_JS.replace("{APPID}", &appid.to_string()), std::time::Duration::from_secs(30))?;
        let bad = || crate::Error::Protocol("invalid install preflight response".into());
        if value["schema"] != 2 { return Err(bad()); }
        let eulas = value["eulas"].as_array().ok_or_else(bad)?.iter().map(|e| {
            let id = e["id"].as_str().filter(|s| !s.trim().is_empty()).map(str::to_owned).or_else(|| e["id"].as_u64().map(|n| n.to_string())).ok_or_else(bad)?;
            let version = e["version"].as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(bad)?;
            let url = e["url"].as_str().filter(|s| !s.is_empty()).ok_or_else(bad)?.to_owned();
            Ok(Eula { id, version, url })
        }).collect::<crate::Result<Vec<_>>>()?;
        Ok(PreflightData { folders: native_folders(&value["folders"]).map_err(|e| crate::Error::Protocol(e.into()))?, required: safe_bytes(&value["requiredBytes"]), eulas })
    }
    pub fn load_eula_content(&self, url: &str) -> crate::Result<String> {
        self.eval_own(&include_str!("eula_load.js").replace("{URL}", &serde_json::to_string(url).unwrap()), std::time::Duration::from_secs(30))?
            .as_str().filter(|s| !s.trim().is_empty()).map(str::to_owned)
            .ok_or_else(|| crate::Error::Protocol("empty EULA content".into()))
    }
    pub fn accept_eula(&self, appid: u32, id: &str, version: u32) -> crate::Result<()> {
        let script = include_str!("eula_accept.js").replace("{APPID}", &appid.to_string()).replace("{ID}", &serde_json::to_string(id).unwrap()).replace("{VERSION}", &version.to_string());
        if self.eval_own(&script, std::time::Duration::from_secs(15))? != Value::Bool(true) { return Err(crate::Error::Protocol("EULA acceptance unconfirmed".into())); }
        Ok(())
    }
    pub fn install_start_headless(&self, appid: u32, folder_index: u32) -> crate::Result<bool> {
        let timeout = std::env::var("DECKSHELL_INSTALL_TIMEOUT_MS").ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(10000).clamp(1500, 20000);
        let script = include_str!("install_headless.js").replace("{APPID}", &appid.to_string()).replace("{FOLDER}", &folder_index.to_string()).replace("{TIMEOUT}", &timeout.to_string());
        Ok(self.eval_own(&script, std::time::Duration::from_millis(u64::from(timeout) + 5000))? == Value::Bool(true))
    }
}

#[cfg(test)]
mod precision_tests {
    use super::*;
    #[test]
    fn install_manager_unsafe_js_bytes_are_unknown() {
        for field in ["nDiskSpaceRequired", "nDiskSpaceAvailable"] {
            let mut v = serde_json::json!({"nDiskSpaceRequired":100,"nDiskSpaceAvailable":200});
            v[field] = serde_json::json!(9_007_199_254_740_992_u64);
            assert_eq!(Space::from_manager(&v).status(), SpaceStatus::Unknown);
        }
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn install_compatibility_preserves_missing_data_and_all_status_codes() {
        for (code, expected) in [
            (0, DeckCategory::Unknown),
            (1, DeckCategory::Unsupported),
            (2, DeckCategory::Playable),
            (3, DeckCategory::Verified),
        ] {
            let report = DeckCompatibility::from_response(&json!({"resolved_category": code, "resolved_items": []}));
            assert_eq!(report.category, Some(expected));
            assert_eq!(report.items, Some(vec![]));
        }
        for response in [json!(null), json!({}), json!({"resolved_category": 5, "resolved_items": "unknown"})] {
            assert_eq!(DeckCompatibility::from_response(&response), DeckCompatibility::default());
        }
        let report = DeckCompatibility::from_response(&json!({
            "resolved_items": [
                {"display_type": 4, "loc_token": "pass"},
                {"display_type": 3, "loc_token": "warning"},
                {"display_type": 2, "loc_token": "fail"},
                {"display_type": 1, "loc_token": "note"},
                {"display_type": 0, "loc_token": ""},
                {"display_type": 99}
            ]
        }));
        let items = report.items.unwrap();
        assert_eq!(
            items.iter().map(|item| item.result).collect::<Vec<_>>(),
            [DeckTestResult::Pass, DeckTestResult::Warning, DeckTestResult::Fail,
                DeckTestResult::Note, DeckTestResult::Unknown, DeckTestResult::Unknown]
        );
        assert_eq!(items[0].token.as_deref(), Some("pass"));
        assert_eq!(items[4].token, None);
        assert_eq!(items[5].token, None);
    }
}

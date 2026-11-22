//! Native install views. Preflight and folder selection never mutate Steam.
use steam_bridge::install::{DeckCompatibility, Space, SpaceStatus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub index: u32, pub label: String, pub mounted: Option<bool>,
    pub free: Option<u64>, pub default: Option<bool>,
}
impl Folder {
    pub fn from_native(f: steam_bridge::install::NativeFolder) -> Self {
        Self { index: f.index, label: f.user_label.filter(|s| !s.trim().is_empty()).unwrap_or(f.drive_name),
            mounted: f.mounted, free: f.free, default: f.default }
    }
    pub fn usable(&self, required: Option<u64>) -> bool {
        self.mounted == Some(true) && Space { required, available: self.free }.status() == SpaceStatus::Sufficient
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eula { pub id: String, pub version: String, pub text: String }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config { pub folders: Vec<Folder>, pub required: Option<u64>, pub selected: Option<u32> }
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum View { #[default] Closed, Compatibility(DeckCompatibility), Config(Config), Eula(Eula), Waiting, Error(String), Unknown(String) }
#[derive(Default)]
pub struct Wizard { pub appid: u32, pub view: View }
impl Wizard {
    pub fn can_continue(&self) -> bool {
        matches!(&self.view, View::Config(c) if c.selected.is_some_and(|i|
            c.folders.iter().filter(|f| f.index == i).count() == 1 && c.folders.iter().any(|f| f.index == i && f.usable(c.required))))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn install_storage_requires_mounted_unique_folder_and_known_sufficient_space() {
        let f = Folder { index: 7, label: "Storage".into(), mounted: Some(true), free: Some(100), default: None };
        let mut c = Config { folders: vec![f.clone()], required: Some(100), selected: Some(7) };
        let check = |c| Wizard { appid: 42, view: View::Config(c) }.can_continue();
        assert!(check(c.clone()));
        c.required = Some(101); assert!(!check(c.clone()));
        c.required = None; assert!(!check(c.clone()));
        c.required = Some(100); c.folders.push(f); assert!(!check(c.clone()));
        c.folders.pop(); c.folders[0].mounted = None; assert!(!check(c));
    }
}

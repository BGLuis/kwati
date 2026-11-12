use gpui::AssetSource;
use kwati_gui::components::{Assets, IconName};

// gpui looks icons up by path at paint time; a path the asset source does not serve
// renders nothing, silently.
#[test]
fn every_icon_is_served_at_its_path() {
    for name in IconName::ALL {
        let bytes = Assets
            .load(name.path())
            .unwrap()
            .unwrap_or_else(|| panic!("{name:?} is not served at {}", name.path()));
        assert!(bytes.starts_with(b"<svg"), "{name:?} is not an SVG");
    }
}

#[test]
fn unknown_paths_load_nothing() {
    assert!(Assets.load("icons/missing.svg").unwrap().is_none());
}

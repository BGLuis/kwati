use gpui::AssetSource;
use kwati_gui::components::{Assets, IconName, IconPart};

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
fn every_animated_icon_part_is_served_at_its_path() {
    for part in IconPart::ALL {
        let bytes = Assets
            .load(part.path())
            .unwrap()
            .unwrap_or_else(|| panic!("{part:?} is not served at {}", part.path()));
        assert!(bytes.starts_with(b"<svg"), "{part:?} is not an SVG");
    }
}

#[test]
fn unknown_paths_load_nothing() {
    assert!(Assets.load("icons/missing.svg").unwrap().is_none());
    assert!(Assets
        .load("stroke:bad:icons/folder.svg")
        .unwrap()
        .is_none());
    assert!(Assets
        .load("stroke:9.0:icons/folder.svg")
        .unwrap()
        .is_none());
}

#[test]
fn every_builtin_svg_uses_refined_stroke_and_scales_via_stroke_prefix() {
    for name in IconName::ALL {
        let bytes = Assets.load(name.path()).unwrap().unwrap();
        let svg = std::str::from_utf8(&bytes).unwrap();
        assert!(
            svg.contains("stroke-width=\"1.15\""),
            "{name:?} should use 1.15 stroke-width"
        );
    }

    let scaled_bytes = Assets
        .load("stroke:0.76:icons/folder.svg")
        .unwrap()
        .unwrap();
    let scaled_svg = std::str::from_utf8(&scaled_bytes).unwrap();
    assert!(scaled_svg.contains("stroke-width=\"0.76\""));
    assert!(!scaled_svg.contains("stroke-width=\"1.15\""));
}

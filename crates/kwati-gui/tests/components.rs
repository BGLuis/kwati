use kwati_gui::components::ScrollbarState;

#[test]
fn scrollbar_state_metrics() {
    let mut state = ScrollbarState::new(100, 20);
    assert!(state.can_scroll());
    assert_eq!(state.scroll_ratio(), 0.0);
    assert!((state.thumb_ratio() - 0.2).abs() < 0.001);

    state.set_metrics(100, 20, 40);
    assert_eq!(state.current_offset, 40);
    assert!((state.scroll_ratio() - 0.5).abs() < 0.001);

    // Bounded at max offset (100 - 20 = 80)
    state.set_metrics(100, 20, 200);
    assert_eq!(state.current_offset, 80);
    assert_eq!(state.scroll_ratio(), 1.0);

    // Non-scrollable when total <= visible
    state.set_metrics(10, 20, 0);
    assert!(!state.can_scroll());
    assert_eq!(state.scroll_ratio(), 0.0);
}

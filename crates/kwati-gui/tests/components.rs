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

#[gpui::test]
fn a_password_field_hides_its_text_from_the_clipboard(cx: &mut gpui::TestAppContext) {
    use gpui::ClipboardItem;
    use kwati_gui::components::{register_text_input_keybindings, TextInput};

    cx.update(register_text_input_keybindings);
    let (input, cx) = cx.add_window_view(|_, cx| {
        let mut input = TextInput::password("Password", cx);
        input.set_text("s3cret", cx);
        input
    });
    input.update_in(cx, |input, window, cx| {
        input.focus_handle.focus(window);
        assert!(input.masked);
        let _ = cx;
    });
    cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
    cx.simulate_keystrokes("ctrl-a ctrl-c ctrl-x");
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("before".into()),
        "neither copy nor cut reaches the clipboard"
    );
    input.read_with(cx, |input, _| assert_eq!(input.text(), "s3cret"));
}

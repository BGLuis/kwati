//! Navigation actions (owned by guinav-worker for #9, #13, #25).

use gpui::actions;

actions!(
    navigation,
    [
        GoUp,
        GoBack,
        GoForward,
        OpenSelected,
        SelectNext,
        SelectPrevious,
        SelectFirst,
        SelectLast,
        Refresh,
    ]
);

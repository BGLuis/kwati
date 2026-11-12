//! File operations and undo actions (owned by guiops-worker for #17, #19, #21, #23, #35, #65).

use gpui::actions;

actions!(
    operations,
    [Copy, Cut, Paste, Delete, Trash, Rename, NewFolder, Undo,]
);

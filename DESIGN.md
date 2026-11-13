# Kwatí design standard

Kwatí is a dense, keyboard-first file explorer. The interface is a tool, not a
showcase: compact rows, thin borders, no decoration that does not carry
information. Its identity comes from the coati (*Nasua nasua*, "quati"; the
name comes from the Tupi *kwa'ti*, "pointed nose"): an earthy palette taken
from its coat, and the animal itself only where the app has nothing else to
show.

## Principles

1. **Content first.** The file list is the product. Chrome stays quiet so
   names, sizes, and dates are what the eye lands on.
2. **Dense, not cramped.** Small type and tight rows, with alignment and
   consistent spacing doing the work that padding would otherwise do.
3. **Earthy, not brown.** Surfaces are warm neutrals; saturated colour is
   reserved for selection, focus, and state.
4. **The coati is a guest.** It appears in the app icon and in empty or error
   states, never in the working chrome.
5. **Every pointer action has a keyboard path**, and focus is always visible.
6. **Every action gets a response.** Hover, press, selection, and each file
   operation are acknowledged with an expressive, springy animation, identical
   on every platform.

## Colour

Both themes carry the same weight. The app follows the operating system
setting by default and offers a manual override (system, light, dark) in
settings.

| Token | Light | Dark | Use |
|---|---|---|---|
| `bg` | `#F4EEE4` | `#1C1714` | Window background, title bar, status bar |
| `surface` | `#FBF7F0` | `#231D19` | File list, preview, text inputs |
| `surface-raised` | `#ECE3D5` | `#2C2420` | Sidebar, column headers, menus, hovered row |
| `border` | `#D9CCB8` | `#3D332C` | Dividers between panes and rows of chrome |
| `border-strong` | `#8C7B69` | `#8A7A6B` | Input outlines, scrollbar thumb, disabled icons |
| `text` | `#2B211A` | `#EDE4D8` | Primary text and icons |
| `text-muted` | `#6B5B4D` | `#A8998A` | Sizes, dates, secondary labels, placeholder |
| `accent` | `#A8481F` | `#E0824F` | Focus ring, primary button, folder icons, links |
| `on-accent` | `#FFF8F0` | `#1C1714` | Text and icons on an `accent` fill |
| `selection` | `#EBCDB6` | `#4A2E20` | Selected rows, selected text |
| `secondary` | `#3F6B4A` | `#7FB08A` | Success, connected, completed transfer |
| `danger` | `#B3261E` | `#F2857C` | Permanent deletion, errors, dropped connection |
| `warning` | `#8A5A00` | `#E0B050` | Timeouts, retrying, conflicts awaiting a choice |

Rules:

- `text`, `text-muted`, `accent`, `secondary`, `danger`, and `warning` all
  reach at least 4.5:1 on `bg`, `surface`, and `surface-raised` in both themes.
- On a `selection` background, use `text` for every column. `text-muted` and
  `accent` fall below 4.5:1 there.
- `border-strong` reaches 3:1 on the surfaces and is for non-text shapes only.
- Colour never carries meaning alone: a state colour is always paired with an
  icon or a word.
- `accent` (rust) is the coati's coat and `secondary` (forest green) is its
  habitat. No other hues enter the interface.

## Typography

- **Interface:** the system sans-serif of each platform. No font is bundled.
- **Data:** the system monospace, for paths, sizes, dates, permissions, hosts,
  and the text preview. Numeric columns are right-aligned.

| Role | Size | Weight | Use |
|---|---|---|---|
| `body` | 13 px | regular | File names, menu items, form fields |
| `data` | 12 px mono | regular | Sizes, dates, paths, status bar |
| `label` | 11 px | medium | Column headers, sidebar section titles |
| `title` | 15 px | semibold | Dialog titles, settings section titles |

Weights are limited to regular, medium, and semibold. Sentence case
everywhere; no all-caps labels.

## Spacing and shape

- Spacing scale: 2, 4, 8, 12, 16, 24 px.
- List row height: 24 px. Sidebar row: 26 px. Toolbar and path bar: 32 px.
  Status bar: 22 px.
- Horizontal padding inside rows and controls: 8 px. Icon-to-label gap: 8 px.
- Corner radius: 4 px for controls, menus, and selected rows; 6 px for
  dialogs. Panes have square corners.
- Borders are 1 px. Panes are separated by a border, not by a gap or a shadow.
- Shadows exist only on floating layers (menus, dialogs, drag preview).
- Focus ring: 1 px `accent`, inset, on the focused control or row.

## Icons

- Monochrome line icons, 16 px, 1.5 px stroke, rounded caps and joins.
- Icons take the colour of their text (`text` or `text-muted`).
- Folder icons use `accent`; this is the only coloured file-type icon.
- File types are told apart by icon shape, not by colour.
- State icons (connected, error, warning) use the matching state colour and
  always sit next to a label.

## Mascot

The coati is drawn flat and geometric: simple solid shapes, no outline, no
gradient, no texture. Three features make it recognisable and must survive at
any size:

- the **ringed tail**, held upright;
- the **long pointed snout** with a light tip;
- the **light mask** around the eyes.

It uses only palette colours: rust body (`accent`), dark-brown rings and
paws (light-theme `text`), cream snout and mask (light-theme `surface`).

Where it appears:

| Place | Form |
|---|---|
| App icon | Head and upright tail, or the ringed tail alone at 16 px |
| About screen | Full figure |
| Empty folder | Small figure, with one line of text |
| Search with no results | Figure sniffing the ground |
| Connection failed or mount unreachable | Figure with a drooping tail |
| Empty trash | Small figure |

Where it does not appear: toolbar, sidebar, list rows, menus, dialogs that ask
for a decision, progress indicators, cursors. It is never animated beyond a
single entrance fade, and it never speaks in the first person.

Illustrations in empty states are at most 96 px tall and are followed by one
sentence and, where it helps, one action.

## Components

- **File list:** one line per entry; name column flexes, size and date are
  fixed-width mono columns. Hover uses `surface-raised`, selection uses
  `selection`, and the focused row carries the focus ring.
- **Sidebar:** `surface-raised` background, `label` section titles, the active
  place marked with `selection`. Dragged near the window edge, or when the
  window is too narrow, it collapses to icons that name themselves in a
  tooltip.
- **Panes:** the sidebar and preview resize by dragging their inner edge. The
  preview closes and opens from the toolbar button or Alt+P, or closes when its
  edge is dragged near the window edge; it reopens at its last width. When
  the window narrows, the file list keeps at least 480 px: the preview narrows
  first, then the sidebar, then the sidebar collapses to icons, and only then
  the preview hides.
- **Path bar:** mono text; segments are clickable and the whole bar turns into
  a text input on click or shortcut.
- **Buttons:** one primary button per dialog (`accent` fill, `on-accent`
  text). Others are borderless text on hover `surface-raised`. A destructive
  confirmation uses a `danger` fill.
- **Jobs panel:** a 2 px progress bar in `accent`, turning `secondary` on
  completion and `danger` on failure, each with its text status.
- **Dialogs:** `surface` background, 6 px radius, title in `title`, actions
  right-aligned with the primary action last.

## Writing

Interface text is short and literal: it names the object and the action
("Delete 3 items permanently?"), states what happened and what to do next in
errors, and avoids exclamation marks and jokes, including in mascot states.

## Motion

Every user action gets a visible response. Motion is feedback: it confirms
that an input was received, shows where something came from or went, and
never makes the user wait. The same durations and curves apply on Linux,
Windows, and macOS, so the app feels the same everywhere.

| Token | Duration | Curve | Use |
|---|---|---|---|
| `instant` | 160 ms | spring | Hover, press, focus ring, selection |
| `quick` | 300 ms | spring | Menus, tooltips, inline rename, toggles, icons |
| `settle` | 460 ms | spring | Dialogs, panels, sidebar collapse, directory change, row insert |

The motion is expressive: movement overshoots its target slightly and settles
back. The `spring` curve is `cubic-bezier(0.34, 1.56, 0.64, 1)`. Colour and
opacity use plain ease-out, since they cannot overshoot. Exits accelerate
away (`cubic-bezier(0.5, 0, 0.75, 0)`) and are a little shorter than
entrances. Groups of items enter in a cascade, 38 ms apart, capped at the
first 12 items so a long list never takes more than half a second to appear.

Responses by interaction:

| Interaction | Response |
|---|---|
| Hover on a row or control | Background fades to `surface-raised` and the icon reacts (`instant`) |
| Press on a button or row | The control scales to 0.93 and springs back on release (`instant`) |
| Focus moves | The focus ring springs in on the new target (`instant`) |
| Select or deselect | `selection` background fades in or out (`instant`) |
| Open a folder, go back or forward | Rows enter in a cascade, sliding 36 px from the direction of travel (`settle`) |
| Sort or filter | Rows slide to their new positions (`settle`) |
| Entry created, renamed, or pasted | The row slides in from the left, growing from 0.85, and flashes `accent` once (`settle`) |
| Entry deleted or moved away | The row is thrown 64 px to the right with a slight tilt, then the rows below close the gap (420 ms) |
| Cut | The cut rows drop to 50 % opacity (`instant`) |
| Drag | Dragged rows lift and dim, a preview follows the pointer, and the drop target pulses with an `accent` outline |
| Menu or tooltip | Springs open from 0.6 at the point it was opened from; menu items cascade 45 ms apart (`quick`) |
| Dialog | Rises 28 px and springs from 0.72 while the window behind dims (`settle`) |
| Panel or sidebar open and close | Width or height springs and the neighbouring pane resizes with it (`settle`) |
| Toggle or checkbox | The knob or check mark springs between states (`quick`) |
| Copy, move, or transfer progress | A 4 px bar springs between updates; on completion it turns `secondary`, pulses once, and the status text pops in |
| Rejected action or invalid input | The field border turns `danger` and shakes 11 px with a slight tilt, five times, decaying (560 ms) |
| Connecting or waiting on a mount | An indeterminate bar appears only after 300 ms of waiting |

Animated icons:

Icons react to hover and to the action they stand for, once per trigger, in
`quick`. They never loop while idle.

| Icon | Reaction |
|---|---|
| Folder | The flap opens on hover and when the folder is entered; closes on leave |
| File | The folded corner lifts on hover |
| Trash | The lid lifts on hover and drops shut after a delete |
| Copy, cut, paste | The two sheets separate on hover; scissors snip once on cut |
| Back, forward, up | The arrow nudges 3 px in its direction on hover and press |
| Search | The lens tilts on hover and while a search runs |
| Refresh, reconnect | Rotates one full turn per trigger |
| Eject | The triangle hops up once |
| Connection state | The mark draws itself in when the state changes |
| Settings | The gear turns a sixth of a turn |

Limits:

- Animation never delays input. Typing, navigating, and a second click act
  immediately and interrupt whatever is still animating. With durations this
  long the rule is not optional: keyboard navigation through several folders
  must never queue animations.
- Text never moves while the user is reading it: a row animates only when it
  enters, leaves, or is acted on. Scrolling is never eased by the app.
- Only the icon under the pointer or tied to the current action animates.
  Icons in list rows do not animate when rows merely scroll into view.
- Changes that come from outside (the directory watcher, a remote poll) use
  the same row insert and remove responses, without the `selection` flash.
- When the system asks for reduced motion, movement and scale are removed and
  only colour and opacity changes of `instant` duration remain.
